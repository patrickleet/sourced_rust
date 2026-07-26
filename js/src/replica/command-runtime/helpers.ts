import {
	isDistributedTrustedPresetCodec,
	parseGraphqlResponseExtensions,
	type DistributedCommandMetadata,
	type DistributedCommandState,
	type DistributedProtocolEnvelope,
	type DistributedRecordRevision,
	type DistributedTrustedPreset
} from '../../protocol.js';
import type { GqlError } from '../../types.js';
import {
	matchReplicaTrustedPresetInventory,
	prepareReplicaCommandWithTrustedPresets,
	verifyReplicaCommandReceipt,
	type ReplicaCommandArtifact,
	type ReplicaPreparedCommand,
	type ReplicaPreparedCommandEffect,
	type ReplicaPreparedEffectKey,
	type ReplicaTrustedPresetDescriptor
} from '../commands.js';
import { replicaRecordKey } from '../identity.js';
import type { ReplicaIndexSemanticChange } from '../index-maintenance.js';
import type {
	DistributedReplica,
	ReplicaAuthoritativeScope,
	ReplicaBaseWriter,
	ReplicaClientSurface,
	ReplicaIdentity,
	ReplicaModelArtifact,
	ReplicaOptimisticWriter,
	ReplicaResultEnvelope,
	ReplicaValue
} from '../types.js';
import {
	INITIAL_STATUS_POLL_MS,
	MAX_OUTPUT_DEPTH,
	MAX_STATUS_POLL_MS,
	MAX_TRANSPORT_RETRIES,
	SHA256
} from './constants.js';
import { ReplicaCommandRuntimeError } from './errors.js';
import { replicaCommandDirectProjection } from './symbols.js';
import type {
	AnyCommandArtifact,
	CapturedAuthority,
	CommandEntry,
	CommandStatusTracker,
	PendingProjection,
	ReplicaBoundCommand,
	ReplicaCommandAuthorityHost,
	ReplicaCommandCallOptions,
	ReplicaCommandDirectProjection,
	ReplicaCommandProjectedOutcome,
	ReplicaCommandReceipt,
	ReplicaCommandStatus,
	ReplicaCommandStatusArtifact,
	ReplicaCommandStatusRequest,
	ReplicaCommandSurfaceContract,
	ReplicaCommandTransport,
	ReplicaCommandTransportRequest,
	ReplicaCommandTransportResult,
	SemanticReplica
} from './types.js';

export function defineBoundCommand(
	root: Record<string, unknown>,
	path: string,
	command: unknown
): void {
	const segments = commandPathSegments(path);
	let namespace = root;
	for (let index = 0; index < segments.length; index += 1) {
		const segment = segments[index]!;
		const leaf = index === segments.length - 1;
		const exists = Object.prototype.hasOwnProperty.call(namespace, segment);
		if (leaf) {
			if (exists) commandNamespaceCollision(path);
			Object.defineProperty(namespace, segment, {
				enumerable: true,
				configurable: false,
				writable: false,
				value: command
			});
			continue;
		}
		if (exists) {
			const value = namespace[segment];
			if (!isPlainRecord(value)) commandNamespaceCollision(path);
			namespace = value as Record<string, unknown>;
			continue;
		}
		const child = Object.create(null) as Record<string, unknown>;
		Object.defineProperty(namespace, segment, {
			enumerable: true,
			configurable: false,
			writable: false,
			value: child
		});
		namespace = child;
	}
}

export function commandPathSegments(path: string): readonly string[] {
	if (path.length === 0 || path.length > 512) {
		throw new TypeError('replica command path is invalid');
	}
	const segments = path.split('.');
	if (
		segments.length > 64 ||
		segments.some(
			(segment) =>
				segment.length === 0 ||
				segment.length > 128 ||
				segment.trim() !== segment ||
				/[\u0000-\u001f\u007f-\u009f]/.test(segment) ||
				segment === '__proto__' ||
				segment === 'prototype' ||
				segment === 'constructor'
		)
	) {
		throw new TypeError(`replica command path ${path} is invalid`);
	}
	return Object.freeze(segments);
}

export function commandNamespaceCollision(path: string): never {
	throw new TypeError(`replica command namespace collision at ${path}`);
}

export function freezeCommandTree(value: Record<string, unknown>): void {
	for (const child of Object.values(value)) {
		if (isPlainRecord(child)) {
			freezeCommandTree(child as Record<string, unknown>);
		}
	}
	Object.freeze(value);
}

export function normalizeInventory<TEntries extends Readonly<Record<string, CommandEntry>>>(
	entries: TEntries
): readonly { readonly key: string; readonly artifact: AnyCommandArtifact }[] {
	if (entries === null || typeof entries !== 'object' || Array.isArray(entries)) {
		throw new TypeError('replica command inventory must be an object');
	}
	const names = new Set<string>();
	const inventory = Object.entries(entries).map(([key, entry]) => {
		commandPathSegments(key);
		const artifact = 'artifact' in entry ? entry.artifact : entry;
		if (names.has(artifact.name)) {
			throw new TypeError(`duplicate replica command artifact ${artifact.name}`);
		}
		names.add(artifact.name);
		return Object.freeze({ key, artifact });
	});
	const sortedPaths = inventory.map(({ key }) => key).sort(compareCodeUnits);
	for (let index = 1; index < sortedPaths.length; index += 1) {
		const previous = sortedPaths[index - 1]!;
		const current = sortedPaths[index]!;
		if (current.startsWith(`${previous}.`)) {
			commandNamespaceCollision(current);
		}
	}
	return Object.freeze(inventory);
}

export function commandSurfaceContract(
	artifacts: readonly AnyCommandArtifact[],
	surfacePresets: readonly ReplicaTrustedPresetDescriptor[] | undefined
): ReplicaCommandSurfaceContract {
	if (artifacts.length === 0) {
		throw new TypeError('replica command inventory must not be empty');
	}
	const first = artifacts[0]!;
	const protocol = first.protocol;
	if (protocol.surface === undefined) {
		throw new TypeError('generated command protocol requires a client surface');
	}
	const trustedPresets = normalizePresetDescriptors(
		protocol.trustedPresets,
		'artifact.protocol.trustedPresets'
	);
	const commandPresets = new Map<string, ReplicaTrustedPresetDescriptor>();
	for (const artifact of artifacts) {
		if (
			artifact.protocol.version !== 2 ||
			artifact.protocol.schemaHash !== protocol.schemaHash ||
			artifact.protocol.protocolHash !== protocol.protocolHash ||
			!sameSurface(artifact.protocol.surface, protocol.surface) ||
			!samePresetDescriptors(
				normalizePresetDescriptors(
					artifact.protocol.trustedPresets,
					'artifact.protocol.trustedPresets'
				),
				trustedPresets
			)
		) {
			throw new TypeError(
				'replica command inventory spans incompatible client surfaces'
			);
		}
		for (const descriptor of artifact.trustedPresets ?? []) {
			const previous = commandPresets.get(descriptor.name);
			if (previous !== undefined && previous.codec !== descriptor.codec) {
				throw new TypeError(
					`trusted preset ${descriptor.name} has conflicting codecs`
				);
			}
			commandPresets.set(
				descriptor.name,
				Object.freeze({ name: descriptor.name, codec: descriptor.codec })
			);
		}
	}
	if (
		surfacePresets !== undefined &&
		!samePresetDescriptors(
			normalizePresetDescriptors(
				surfacePresets,
				'status.protocol.trustedPresets'
			),
			trustedPresets
		)
	) {
		throw new TypeError(
			'generated command status inventory does not match its client surface'
		);
	}
	const surfaceByName = new Map(
		trustedPresets.map((descriptor) => [descriptor.name, descriptor] as const)
	);
	for (const descriptor of commandPresets.values()) {
		if (surfaceByName.get(descriptor.name)?.codec !== descriptor.codec) {
			throw new TypeError(
				`command trusted preset ${descriptor.name} is absent from the client surface`
			);
		}
	}
	return Object.freeze({
		protocolVersion: 2,
		schemaHash: protocol.schemaHash,
		protocolHash: protocol.protocolHash,
		surface: cloneSurface(protocol.surface),
		trustedPresets
	});
}

export function commandStatusArtifact(
	value: ReplicaCommandStatusArtifact,
	contract: ReplicaCommandSurfaceContract
): ReplicaCommandStatusArtifact {
	if (
		value === null ||
		typeof value !== 'object' ||
		typeof value.name !== 'string' ||
		value.name.trim().length === 0 ||
		typeof value.document !== 'string' ||
		value.document.trim().length === 0 ||
		typeof value.operationHash !== 'string' ||
		!SHA256.test(value.operationHash) ||
		value.protocol === null ||
		typeof value.protocol !== 'object' ||
		value.protocol.version !== 2 ||
		value.protocol.operation !== value.operationHash ||
		value.protocol.schemaHash !== contract.schemaHash ||
		value.protocol.protocolHash !== contract.protocolHash ||
		!sameSurface(value.protocol.surface, contract.surface)
	) {
		throw new TypeError('generated command status artifact is invalid');
	}
	const trustedPresets = normalizePresetDescriptors(
		value.protocol.trustedPresets,
		'status.protocol.trustedPresets'
	);
	if (!samePresetDescriptors(trustedPresets, contract.trustedPresets)) {
		throw new TypeError(
			'generated command status inventory does not match its client surface'
		);
	}
	return Object.freeze({
		name: value.name,
		document: value.document,
		operationHash: value.operationHash,
		protocol: Object.freeze({
			version: 2,
			schemaHash: contract.schemaHash,
			protocolHash: contract.protocolHash,
			surface: cloneSurface(contract.surface),
			operation: value.operationHash,
			trustedPresets
		})
	});
}

export function normalizePresetDescriptors(
	value: readonly ReplicaTrustedPresetDescriptor[],
	path: string
): readonly ReplicaTrustedPresetDescriptor[] {
	if (!Array.isArray(value)) {
		throw new TypeError(`${path} must be an array`);
	}
	const names = new Set<string>();
	const result = value.map((descriptor, index) => {
		if (
			descriptor === null ||
			typeof descriptor !== 'object' ||
			typeof descriptor.name !== 'string' ||
			descriptor.name.length === 0 ||
			descriptor.name.length > 128 ||
			descriptor.name.trim() !== descriptor.name ||
			/[\u0000-\u001f\u007f-\u009f]/.test(descriptor.name) ||
			names.has(descriptor.name) ||
			!isDistributedTrustedPresetCodec(descriptor.codec)
		) {
			throw new TypeError(`${path}[${index}] is invalid`);
		}
		names.add(descriptor.name);
		return Object.freeze({
			name: descriptor.name,
			codec: descriptor.codec
		});
	});
	return Object.freeze(
		result.sort(({ name: left }, { name: right }) =>
			compareCodeUnits(left, right)
		)
	);
}

export function samePresetDescriptors(
	left: readonly ReplicaTrustedPresetDescriptor[],
	right: readonly ReplicaTrustedPresetDescriptor[]
): boolean {
	return (
		left.length === right.length &&
		left.every(
			(descriptor, index) =>
				descriptor.name === right[index]?.name &&
				descriptor.codec === right[index]?.codec
		)
	);
}

export function applyOptimisticEffects(
	writer: ReplicaOptimisticWriter,
	effects: readonly ReplicaPreparedCommandEffect[]
): void {
	for (const effect of effects) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch': {
				const model = modelFromKey(effect.model, effect.key);
				writer.writeRecord(model, identityFromKey(effect.key), {
					fields: fieldsFromEffect(effect.model, effect.key, effect.fields)
				});
				break;
			}
			case 'delete':
				writer.tombstoneRecord(
					modelFromKey(effect.model, effect.key),
					identityFromKey(effect.key)
				);
				break;
			case 'link':
			case 'unlink':
			case 'invalidate_model':
			case 'invalidate_relationship':
				// Task 8 consumes the exact semantic context. Guessing a to-one
				// record link for a to-many relationship would corrupt truth.
				break;
		}
	}
}

export function preparedSemanticChanges<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>
): readonly ReplicaIndexSemanticChange[] {
	const dependencies = Object.freeze([...prepared.revalidation.dependencies]);
	const changes: ReplicaIndexSemanticChange[] = [];
	for (const effect of prepared.optimistic.operations) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch':
			case 'delete':
				// DistributedReplica captures ordinary writer mutations into the
				// same layer context. Supplying them again would double-apply the
				// semantic record operation.
				break;
			case 'link':
			case 'unlink': {
				const source = modelFromKey(
					effect.relationship.sourceModel,
					effect.source
				);
				const target = modelFromKey(
					effect.relationship.targetModel,
					effect.target
				);
				changes.push(
					Object.freeze({
						kind: effect.kind,
						sourceModel: effect.relationship.sourceModel,
						field: effect.relationship.field,
						targetModel: effect.relationship.targetModel,
						sourceKey: replicaRecordKey(
							source,
							identityFromKey(effect.source)
						),
						targetKey: replicaRecordKey(
							target,
							identityFromKey(effect.target)
						),
						dependencies
					})
				);
				break;
			}
			case 'invalidate_model':
			case 'invalidate_relationship':
				changes.push(
					Object.freeze({
						kind: 'invalidate',
						dependencies
					})
				);
				break;
		}
	}
	return Object.freeze(changes);
}

export function modelFromKey(
	model: string,
	key: ReplicaPreparedEffectKey
): ReplicaModelArtifact {
	return Object.freeze({
		id: model,
		identityFields: Object.freeze(key.fields.map(({ field }) => field))
	});
}

export function identityFromKey(
	key: ReplicaPreparedEffectKey
): readonly ReplicaValue[] {
	return Object.freeze(key.fields.map(({ value }) => value));
}

export function fieldsFromEffect(
	model: string,
	key: ReplicaPreparedEffectKey,
	fields: readonly { readonly field: string; readonly value: ReplicaValue }[]
): Readonly<Record<string, ReplicaValue>> {
	const result: Record<string, ReplicaValue> = Object.create(null) as Record<
		string,
		ReplicaValue
	>;
	for (const field of [
		{ field: '__typename', value: model },
		...key.fields,
		...fields
	]) {
		Object.defineProperty(result, field.field, {
			enumerable: true,
			configurable: false,
			writable: false,
			value: field.value
		});
	}
	return Object.freeze(result);
}

export function commandTransportRequest<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	signal: AbortSignal | undefined
): ReplicaCommandTransportRequest {
	const surface = prepared.transport.protocol.surface;
	if (surface === undefined) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID',
			{ commandId: prepared.commandId }
		);
	}
	return Object.freeze({
		operation: 'mutation',
		commandName: prepared.name,
		commandId: prepared.commandId,
		mutationField: prepared.transport.mutationField,
		document: prepared.transport.document,
		operationHash: prepared.transport.operationHash,
		variables: prepared.transport.variables as Readonly<Record<string, unknown>>,
		extensions: Object.freeze({
			distributed: Object.freeze({
				client: Object.freeze({
					surface: cloneSurface(surface),
					schemaHash: prepared.transport.protocol.schemaHash
				})
			})
		}),
		...(signal === undefined ? {} : { signal })
	});
}

export function commandStatusRequest(
	artifact: ReplicaCommandStatusArtifact,
	commandId: string,
	signal: AbortSignal | undefined
): ReplicaCommandStatusRequest {
	return Object.freeze({
		operation: 'status',
		commandId,
		name: artifact.name,
		document: artifact.document,
		operationHash: artifact.operationHash,
		variables: Object.freeze({ commandId }),
		extensions: Object.freeze({
			distributed: Object.freeze({
				client: Object.freeze({
					surface: cloneSurface(artifact.protocol.surface),
					schemaHash: artifact.protocol.schemaHash
				})
			})
		}),
		...(signal === undefined ? {} : { signal })
	});
}

export async function dispatchPrepared(
	transport: ReplicaCommandTransport,
	request: ReplicaCommandTransportRequest,
	retries: number,
	onAttempt: () => void
): Promise<ReplicaCommandTransportResult> {
	let error: unknown;
	for (let attempt = 0; attempt <= retries; attempt += 1) {
		if (request.signal?.aborted) {
			throw request.signal.reason ?? new Error('command request aborted');
		}
		try {
			onAttempt();
			return await waitForCommandOperation(
				transport.dispatch(request),
				request.signal
			);
		} catch (candidate) {
			error = candidate;
			if (request.signal?.aborted) throw candidate;
		}
	}
	throw error;
}

export function requireCommandEnvelope<TInput, TOutput>(
	result: ReplicaCommandTransportResult,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	authority: CapturedAuthority
): DistributedProtocolEnvelope {
	const distributed = parseGraphqlResponseExtensions(result.extensions)?.distributed;
	if (
		distributed === undefined ||
		distributed.command === undefined ||
		distributed.operation !== prepared.transport.operationHash ||
		distributed.protocolVersion !== authority.scope.protocolVersion ||
		distributed.schemaHash !== authority.scope.schemaHash ||
		distributed.cacheScope !== authority.scope.cacheScope
	) {
		throw new Error('command response does not match its generated scope');
	}
	verifyReplicaCommandReceipt(prepared, distributed.command);
	return distributed;
}

/**
 * Domain rejection happens before a command receipt exists, so GraphQL cannot
 * attach `distributed.command`. It still has to prove the exact generated
 * operation and authoritative cache scope before the runtime may classify the
 * response as a normal rejection instead of a protocol failure.
 */
export function requireCommandRejectionEnvelope<TInput, TOutput>(
	result: ReplicaCommandTransportResult,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	authority: CapturedAuthority
): DistributedProtocolEnvelope {
	const distributed = parseGraphqlResponseExtensions(result.extensions)?.distributed;
	if (
		!Number.isSafeInteger(result.status) ||
		result.status < 200 ||
		result.status >= 300 ||
		result.data !== null ||
		graphqlCommandRejection(result) === undefined ||
		distributed === undefined ||
		distributed.command !== undefined ||
		distributed.operation !== prepared.transport.operationHash ||
		distributed.protocolVersion !== authority.scope.protocolVersion ||
		distributed.schemaHash !== authority.scope.schemaHash ||
		distributed.cacheScope !== authority.scope.cacheScope
	) {
		throw new Error('command rejection does not match its generated scope');
	}
	return distributed;
}

export function graphqlCommandRejection(
	result: ReplicaCommandTransportResult
): GqlError | undefined {
	const errors = result.errors;
	if (
		parseGraphqlResponseExtensions(result.extensions)?.distributed?.command !==
			undefined ||
		errors === undefined ||
		errors.length === 0 ||
		!errors.every((error) => error.extensions?.code === 'REJECTED')
	) {
		return undefined;
	}
	return errors[0];
}

export function requireStatusEnvelope<TInput, TOutput>(
	result: ReplicaCommandTransportResult,
	artifact: ReplicaCommandStatusArtifact,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	authority: CapturedAuthority,
	contract: ReplicaCommandSurfaceContract
): ReplicaCommandStatus {
	if (
		!Number.isSafeInteger(result.status) ||
		result.status < 200 ||
		result.status >= 300 ||
		(result.errors?.length ?? 0) !== 0
	) {
		throw new Error('command status request did not succeed');
	}
	const state = commandStatusOutput(result.data);
	const distributed = parseGraphqlResponseExtensions(result.extensions)?.distributed;
	if (
		distributed === undefined ||
		distributed.operation !== artifact.operationHash ||
		distributed.protocolVersion !== authority.scope.protocolVersion ||
		distributed.schemaHash !== authority.scope.schemaHash ||
		distributed.cacheScope !== authority.scope.cacheScope ||
		distributed.snapshot !== undefined ||
		distributed.live !== undefined
	) {
		throw new Error('command status response does not match its generated scope');
	}
	matchReplicaTrustedPresetInventory(
		contract.trustedPresets,
		distributed.trustedPresets
	);
	const metadata = distributed.command;
	if (metadata === undefined) {
		if (state !== 'unknown' && state !== 'expired') {
			throw new Error('command status omitted required causal metadata');
		}
		return Object.freeze({
			commandId: prepared.commandId,
			state
		});
	}
	verifyReplicaCommandReceipt(prepared, metadata);
	if (metadata.state !== state) {
		throw new Error('command status data and causal metadata disagree');
	}
	return Object.freeze({
		commandId: prepared.commandId,
		state,
		metadata
	});
}

export function commandStatusOutput(
	data: Readonly<Record<string, unknown>> | null | undefined
): DistributedCommandState {
	if (
		data === undefined ||
		data === null ||
		!isPlainRecord(data) ||
		Reflect.ownKeys(data).length !== 1 ||
		!Object.prototype.hasOwnProperty.call(data, 'commandStatus')
	) {
		throw new Error('command status data has an invalid root shape');
	}
	const value = data.commandStatus;
	if (
		!isPlainRecord(value) ||
		Reflect.ownKeys(value).length !== 1 ||
		!Object.prototype.hasOwnProperty.call(value, 'state')
	) {
		throw new Error('command status data has an invalid result shape');
	}
	switch (value.state) {
		case 'in_progress':
		case 'accepted':
		case 'accepted_pending_projection':
		case 'projected':
		case 'rejected':
		case 'projection_failed':
		case 'expired':
		case 'unknown':
			return value.state;
		default:
			throw new Error('command status data has an invalid state');
	}
}

export function validateStatusProgression(
	previousState: DistributedCommandState | undefined,
	previous: DistributedCommandMetadata | undefined,
	current: ReplicaCommandStatus
): void {
	if (
		previousState !== undefined &&
		!isStatusTransition(previousState, current.state)
	) {
		throw new Error('command status regressed or changed terminal outcome');
	}
	const next = current.metadata;
	if (next === undefined) {
		if (
			(current.state !== 'unknown' && current.state !== 'expired') ||
			(previous !== undefined && current.state === 'unknown')
		) {
			throw new Error('command status lost causal metadata');
		}
		return;
	}
	if (next.state !== current.state) {
		throw new Error('command status metadata has an inconsistent state');
	}
	if (previous === undefined) return;
	if (
		next.commandId !== previous.commandId ||
		next.causationId !== previous.causationId ||
		next.consistency !== previous.consistency
	) {
		throw new Error('command status changed causal identity');
	}
	if (
		!(
			previous.state === 'in_progress' &&
			previous.expects.length === 0
		) &&
		!sameStringMultiset(
			previous.expects.map(projectionExpectationFingerprint),
			next.expects.map(projectionExpectationFingerprint)
		)
	) {
		throw new Error('command status changed projection expectations');
	}
	if (
		!isStringSubset(
			previous.observations.map(projectionObservationFingerprint),
			next.observations.map(projectionObservationFingerprint)
		) ||
		!isStringSubset(
			previous.records.map(recordRevisionFingerprint),
			next.records.map(recordRevisionFingerprint)
		)
	) {
		throw new Error('command status lost causal evidence');
	}
}

export function isStatusTransition(
	previous: DistributedCommandState,
	next: DistributedCommandState
): boolean {
	switch (previous) {
		case 'unknown':
			return true;
		case 'in_progress':
			return next !== 'unknown';
		case 'accepted':
			return next === 'accepted' || next === 'expired';
		case 'accepted_pending_projection':
			return (
				next === 'accepted_pending_projection' ||
				next === 'projected' ||
				next === 'projection_failed' ||
				next === 'expired'
			);
		case 'projected':
			return next === 'projected' || next === 'expired';
		case 'rejected':
			return next === 'rejected' || next === 'expired';
		case 'projection_failed':
			return next === 'projection_failed' || next === 'expired';
		case 'expired':
			return next === 'expired';
	}
}

export function projectionExpectationFingerprint(
	value: DistributedCommandMetadata['expects'][number]
): string {
	return tupleFingerprint([
		value.projection,
		value.model,
		value.scopeToken
	]);
}

export function projectionObservationFingerprint(
	value: DistributedCommandMetadata['observations'][number]
): string {
	return tupleFingerprint([
		value.causationId,
		value.projection,
		value.model,
		value.scopeToken
	]);
}

export function recordRevisionFingerprint(
	value: DistributedCommandMetadata['records'][number]
): string {
	return tupleFingerprint([
		value.model,
		value.scopeToken,
		value.incarnation,
		value.revision,
		value.tombstone ? '1' : '0',
		...(value.path ?? [])
	]);
}

export function tupleFingerprint(parts: readonly string[]): string {
	return parts.map((part) => `${part.length}:${part}`).join('');
}

export function sameStringMultiset(
	left: readonly string[],
	right: readonly string[]
): boolean {
	if (left.length !== right.length) return false;
	const sortedLeft = [...left].sort(compareCodeUnits);
	const sortedRight = [...right].sort(compareCodeUnits);
	return sortedLeft.every((value, index) => value === sortedRight[index]);
}

export function isStringSubset(
	subset: readonly string[],
	superset: readonly string[]
): boolean {
	const remaining = new Map<string, number>();
	for (const value of superset) {
		remaining.set(value, (remaining.get(value) ?? 0) + 1);
	}
	for (const value of subset) {
		const count = remaining.get(value) ?? 0;
		if (count === 0) return false;
		remaining.set(value, count - 1);
	}
	return true;
}

export function commandOutput<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	data: Readonly<Record<string, unknown>> | null | undefined,
	field: string
): unknown {
	if (
		data === undefined ||
		data === null ||
		!Object.prototype.hasOwnProperty.call(data, field) ||
		Reflect.ownKeys(data).some((key) => key !== field)
	) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID'
		);
	}
	return cloneOutputShape(artifact.output, data[field], `data.${field}`, 0);
}

export function cloneOutputShape(
	shape: ReplicaCommandArtifact<unknown, unknown>['output'],
	value: unknown,
	path: string,
	depth: number
): unknown {
	if (depth > MAX_OUTPUT_DEPTH) outputInvalid(path);
	if (shape.kind !== 'object') outputInvalid(path);
	if (!isPlainRecord(value)) outputInvalid(path);
	const known = new Set(shape.definition.fields.map(({ name }) => name));
	for (const key of Reflect.ownKeys(value)) {
		if (typeof key !== 'string' || !known.has(key)) outputInvalid(`${path}.${String(key)}`);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (descriptor === undefined || !('value' in descriptor)) {
			outputInvalid(`${path}.${key}`);
		}
	}
	const output: Record<string, unknown> = {};
	for (const field of shape.definition.fields) {
		const present =
			Object.prototype.hasOwnProperty.call(value, field.name) &&
			value[field.name] !== undefined;
		if (!present) {
			outputInvalid(`${path}.${field.name}`);
		}
		const fieldValue = value[field.name];
		if (fieldValue === null) {
			if (!field.nullable) outputInvalid(`${path}.${field.name}`);
			output[field.name] = null;
			continue;
		}
		const cloneItem = (item: unknown, itemPath: string): unknown =>
			field.nested === undefined
				? cloneOutputScalar(field.codec, item, itemPath)
				: cloneOutputShape(
						{ kind: 'object', definition: field.nested },
						item,
						itemPath,
						depth + 1
					);
		if (field.list) {
			if (!Array.isArray(fieldValue)) outputInvalid(`${path}.${field.name}`);
			output[field.name] = Object.freeze(
				fieldValue.map((item, index) => {
					if (item === null) {
						if (!field.itemNullable) {
							outputInvalid(`${path}.${field.name}[${index}]`);
						}
						return null;
					}
					return cloneItem(item, `${path}.${field.name}[${index}]`);
				})
			);
		} else {
			output[field.name] = cloneItem(fieldValue, `${path}.${field.name}`);
		}
	}
	return Object.freeze(output);
}

export function cloneOutputScalar(
	codec: string | undefined,
	value: unknown,
	path: string
): ReplicaValue {
	switch (codec) {
		case 'string':
		case 'string_unvalidated_timestamp':
		case 'base64':
			if (typeof value !== 'string') outputInvalid(path);
			return value;
		case 'boolean':
			if (typeof value !== 'boolean') outputInvalid(path);
			return value;
		case 'int32':
			if (
				typeof value !== 'number' ||
				!Number.isInteger(value) ||
				value < -2_147_483_648 ||
				value > 2_147_483_647
			) {
				outputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'json_number_precision_limited':
			if (typeof value !== 'number' || !Number.isInteger(value)) {
				outputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'float64':
			if (typeof value !== 'number' || !Number.isFinite(value)) {
				outputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'json':
			return cloneOutputJson(value, path, new Set(), 0);
		default:
			outputInvalid(`${path}.codec`);
	}
}

export function cloneOutputJson(
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number
): ReplicaValue {
	if (depth > MAX_OUTPUT_DEPTH) outputInvalid(path);
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean'
	) {
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) outputInvalid(path);
		return Object.is(value, -0) ? 0 : value;
	}
	if (typeof value !== 'object' || active.has(value)) outputInvalid(path);
	active.add(value);
	if (Array.isArray(value)) {
		const output = Object.freeze(
			value.map((item, index) =>
				cloneOutputJson(item, `${path}[${index}]`, active, depth + 1)
			)
		);
		active.delete(value);
		return output;
	}
	if (!isPlainRecord(value)) outputInvalid(path);
	const output: Record<string, ReplicaValue> = {};
	for (const key of Reflect.ownKeys(value).sort(comparePropertyKeys)) {
		if (typeof key !== 'string') outputInvalid(path);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (
			descriptor === undefined ||
			!('value' in descriptor) ||
			descriptor.value === undefined
		) {
			outputInvalid(`${path}.${key}`);
		}
		output[key] = cloneOutputJson(
			descriptor.value,
			`${path}.${key}`,
			active,
			depth + 1
		);
	}
	active.delete(value);
	return Object.freeze(output);
}

export function confirmDirectProjection<TInput, TOutput>(
	replica: ReplicaCommandAuthorityHost,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	output: TOutput,
	metadata: DistributedCommandMetadata
): void {
	const direct = prepared.directProjection;
	if (direct === undefined || !isPlainRecord(output)) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID',
			{ commandId: prepared.commandId }
		);
	}
	const identity: ReplicaValue[] = [];
	for (const field of direct.identityFields) {
		const value = output[field];
		if (value === undefined || value === null) {
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROTOCOL_INVALID',
				{ commandId: prepared.commandId }
			);
		}
		identity.push(value as ReplicaValue);
	}
	const evidence = metadata.records.filter(
		(record) =>
			record.model === direct.model &&
			!record.tombstone &&
			(
				record.path === undefined ||
				(record.path.length === 1 &&
					record.path[0] === prepared.transport.mutationField)
			)
	);
	if (evidence.length !== 1) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID',
			{ commandId: prepared.commandId }
		);
	}
	const record = evidence[0]!;
	const model: ReplicaModelArtifact = Object.freeze({
		id: direct.model,
		identityFields: direct.identityFields
	});
	const fields = cloneOutputJson(
		output,
		'projected.output',
		new Set(),
		0
	) as Readonly<Record<string, ReplicaValue>>;
	const confirmProtocolProjection = replica[replicaCommandDirectProjection];
	if (confirmProtocolProjection !== undefined) {
		confirmProtocolProjection.call(replica, prepared.commandId, {
			model,
			identity: Object.freeze(identity),
			evidence: record,
			fields
		});
		return;
	}
	replica.confirmOptimisticLayer(prepared.commandId, (writer: ReplicaBaseWriter) =>
		writer.writeRecord(model, Object.freeze(identity), record.revision, {
			incarnation: record.incarnation,
			fields
		})
	);
}

export function pendingProjection(
	authority: CapturedAuthority,
	metadata: DistributedCommandMetadata,
	prepared: ReplicaPreparedCommand<unknown, unknown>
): PendingProjection {
	let resolve!: (value: ReplicaCommandProjectedOutcome<unknown>) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<ReplicaCommandProjectedOutcome<unknown>>(
		(resolvePromise, rejectPromise) => {
			resolve = resolvePromise;
			reject = rejectPromise;
		}
	);
	/*
	 * Authority loss or a terminal status can arrive before application code
	 * receives the accepted receipt. Mark the internal lifecycle promise handled
	 * eagerly while preserving its rejection for every explicit awaiter.
	 */
	void promise.catch(() => undefined);
	const controller: PendingProjection = {
		commandId: metadata.commandId,
		causationId: metadata.causationId,
		authority,
		resolve,
		reject,
		promise,
		prepared,
		metadata,
		settled: false
	};
	return controller;
}

/**
 * Generated fact commands converge without application polling. Durable status
 * is the only completion signal; timers merely schedule reads and never infer a
 * successful outcome.
 */
export function monitorPendingProjection(
	controller: PendingProjection,
	readStatus: () => Promise<ReplicaCommandStatus>,
	retained: () => boolean,
	reportError: ((error: unknown) => void) | undefined
): void {
	if (
		controller.settled ||
		!retained() ||
		projectionAuthorityAborted(controller)
	) {
		return;
	}
	const monitorAbort = new AbortController();
	const stopMonitor = () => monitorAbort.abort();
	controller.stopMonitor = stopMonitor;
	const signals =
		controller.authority.signal === undefined
			? [monitorAbort.signal]
			: [controller.authority.signal, monitorAbort.signal];
	void (async () => {
		try {
			let delay = INITIAL_STATUS_POLL_MS;
			while (
				!controller.settled &&
				retained() &&
				!projectionAuthorityAborted(controller)
			) {
				await waitForProjectionPoll(delay, signals);
				if (
					controller.settled ||
					!retained() ||
					projectionAuthorityAborted(controller)
				) {
					return;
				}
				try {
					await readStatus();
				} catch (error) {
					if (
						controller.settled ||
						!retained() ||
						projectionAuthorityAborted(controller)
					) {
						return;
					}
					reportBackgroundErrorSafely(reportError, error);
				}
				delay = Math.min(delay * 2, MAX_STATUS_POLL_MS);
			}
		} finally {
			if (controller.stopMonitor === stopMonitor) {
				controller.stopMonitor = undefined;
			}
			monitorAbort.abort();
		}
	})().catch((error: unknown) =>
		reportBackgroundErrorSafely(reportError, error)
	);
}

export function reportBackgroundErrorSafely(
	reportError: ((error: unknown) => void) | undefined,
	error: unknown
): void {
	if (reportError === undefined) return;
	try {
		reportError(error);
	} catch {
		// Error reporting is a terminal boundary and must never reject detached work.
	}
}

export function projectionAuthorityAborted(
	controller: PendingProjection
): boolean {
	return controller.authority.signal?.aborted === true;
}

export function waitForProjectionPoll(
	delay: number,
	signals: readonly AbortSignal[]
): Promise<void> {
	if (signals.some((signal) => signal.aborted)) return Promise.resolve();
	return new Promise((resolve) => {
		let settled = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const finish = () => {
			if (settled) return;
			settled = true;
			if (timer !== undefined) clearTimeout(timer);
			for (const signal of signals) {
				signal.removeEventListener('abort', finish);
			}
			resolve();
		};
		timer = setTimeout(finish, delay);
		/*
		 * This poll owns the unresolved projected lifecycle. Keep Node's timer
		 * referenced until projection settlement or runtime disposal calls
		 * `finish`, which clears the timer and every abort listener.
		 */
		for (const signal of signals) {
			signal.addEventListener('abort', finish, { once: true });
		}
		// Close the check/register race if either signal aborted synchronously.
		if (signals.some((signal) => signal.aborted)) finish();
	});
}

export function settleProjectionSuccess(controller: PendingProjection): void {
	if (controller.settled) return;
	controller.settled = true;
	controller.abort?.();
	controller.stopMonitor?.();
	controller.resolve(
		Object.freeze({
			commandId: controller.commandId,
			state: 'projected',
			metadata: controller.metadata
		})
	);
}

export function callerProjectedPromise(
	controller: PendingProjection,
	signal: AbortSignal | undefined
): Promise<ReplicaCommandProjectedOutcome<unknown>> {
	if (signal === undefined) return controller.promise;
	const callerSignal = signal;
	const promise = new Promise<ReplicaCommandProjectedOutcome<unknown>>(
		(resolve, reject) => {
			let settled = false;
			function settle(complete: () => void): void {
				if (settled) return;
				settled = true;
				callerSignal.removeEventListener('abort', onAbort);
				complete();
			}
			function onAbort(): void {
				/*
				 * Internal causal settlement wins once selected, even if its
				 * promise callbacks have not run yet. Caller cancellation never
				 * mutates that internal lifecycle.
				 */
				if (controller.settled) return;
				settle(() =>
					reject(
						new ReplicaCommandRuntimeError('REPLICA_COMMAND_ABORTED', {
							commandId: controller.commandId
						})
					)
				);
			}
			callerSignal.addEventListener('abort', onAbort, { once: true });
			void controller.promise.then(
				(value) => settle(() => resolve(value)),
				(error: unknown) => settle(() => reject(error))
			);
			if (callerSignal.aborted) onAbort();
		}
	);
	/*
	 * An AbortSignal can fire during an async `onAccepted` callback, before the
	 * receipt reaches caller code. Keep that legitimate rejection observable
	 * without creating a process-level unhandled-rejection race.
	 */
	void promise.catch(() => undefined);
	return promise;
}

export function attachAuthorityAbort(
	controller: PendingProjection,
	onSettled: () => void
): void {
	const signal = controller.authority.signal;
	if (signal === undefined) return;
	const onAbort = () => {
		settleProjectionFailure(
			controller,
			new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_SCOPE_INVALIDATED',
				{ commandId: controller.commandId }
			)
		);
		onSettled();
	};
	signal.addEventListener('abort', onAbort, { once: true });
	controller.abort = () => signal.removeEventListener('abort', onAbort);
	if (signal.aborted) onAbort();
}

export function settleProjectionFailure(
	controller: PendingProjection,
	error: unknown
): void {
	if (controller.settled) return;
	controller.settled = true;
	controller.abort?.();
	controller.stopMonitor?.();
	controller.reject(error);
}

export function settleTrackedProjection(
	tracker: CommandStatusTracker,
	pending: Map<string, PendingProjection>
): void {
	const controller = tracker.pending;
	if (controller === undefined || controller.settled) return;
	settleProjectionSuccess(controller);
	pending.delete(controller.commandId);
}

export function failTrackedProjection(
	tracker: CommandStatusTracker,
	pending: Map<string, PendingProjection>,
	error: unknown
): void {
	const controller = tracker.pending;
	if (controller === undefined) return;
	settleProjectionFailure(controller, error);
	pending.delete(controller.commandId);
}

export function normalizeRetries(value: number | undefined): number {
	if (value === undefined) return 0;
	if (!Number.isSafeInteger(value) || value < 0 || value > MAX_TRANSPORT_RETRIES) {
		throw new TypeError(
			`transportRetries must be an integer between 0 and ${MAX_TRANSPORT_RETRIES}`
		);
	}
	return value;
}

export function cloneScope(scope: ReplicaAuthoritativeScope): ReplicaAuthoritativeScope {
	return Object.freeze({
		protocolVersion: 2,
		schemaHash: scope.schemaHash,
		cacheScope: scope.cacheScope
	});
}

export function sameScope(
	left: ReplicaAuthoritativeScope,
	right: ReplicaAuthoritativeScope
): boolean {
	return (
		left.protocolVersion === right.protocolVersion &&
		left.schemaHash === right.schemaHash &&
		left.cacheScope === right.cacheScope
	);
}

export function sameSurface(
	left: ReplicaClientSurface | undefined,
	right: ReplicaClientSurface
): boolean {
	if (
		left === undefined ||
		left.kind !== right.kind ||
		left.name !== right.name
	) {
		return false;
	}
	return (
		left.kind === 'role' ||
		(right.kind === 'application' &&
			left.roles.length === right.roles.length &&
			left.roles.every((role, index) => role === right.roles[index]))
	);
}

export function cloneSurface(surface: ReplicaClientSurface): ReplicaClientSurface {
	return surface.kind === 'role'
		? Object.freeze({ kind: 'role', name: surface.name })
		: Object.freeze({
				kind: 'application',
				name: surface.name,
				roles: Object.freeze([...surface.roles])
			});
}

export function linkAbortSignals(
	signals: readonly (AbortSignal | undefined)[]
): Readonly<{
	signal: AbortSignal | undefined;
	dispose(): void;
}> {
	const sources = [
		...new Set(
			signals.filter(
				(signal): signal is AbortSignal => signal !== undefined
			)
		)
	];
	if (sources.length === 0) {
		return Object.freeze({
			signal: undefined,
			dispose(): void {}
		});
	}
	if (sources.length === 1) {
		return Object.freeze({
			signal: sources[0],
			dispose(): void {}
		});
	}
	const controller = new AbortController();
	const listeners = new Map<AbortSignal, () => void>();
	let disposed = false;
	const dispose = (): void => {
		if (disposed) return;
		disposed = true;
		for (const [source, listener] of listeners) {
			source.removeEventListener('abort', listener);
		}
		listeners.clear();
	};
	const abort = (signal: AbortSignal) => {
		if (!controller.signal.aborted) {
			controller.abort(signal.reason);
		}
		dispose();
	};
	for (const source of sources) {
		if (source.aborted) {
			abort(source);
			break;
		}
		const listener = () => abort(source);
		listeners.set(source, listener);
		source.addEventListener('abort', listener, { once: true });
		// Close the check/register race if a source aborted synchronously.
		if (source.aborted) {
			listener();
			break;
		}
	}
	return Object.freeze({ signal: controller.signal, dispose });
}

export function waitForCommandOperation<T>(
	operation: Promise<T> | T,
	signal: AbortSignal | undefined
): Promise<T> {
	const result = Promise.resolve(operation);
	if (signal === undefined) return result;
	return new Promise<T>((resolve, reject) => {
		let settled = false;
		const finish = (complete: () => void): void => {
			if (settled) return;
			settled = true;
			signal.removeEventListener('abort', onAbort);
			complete();
		};
		const onAbort = (): void => {
			finish(() =>
				reject(
					signal.reason ??
						new ReplicaCommandRuntimeError('REPLICA_COMMAND_ABORTED')
				)
			);
		};
		signal.addEventListener('abort', onAbort, { once: true });
		void result.then(
			(value) => finish(() => resolve(value)),
			(error: unknown) => finish(() => reject(error))
		);
		// Close the check/register race if the signal aborted synchronously.
		if (signal.aborted) onAbort();
	});
}

export function isPlainRecord(
	value: unknown
): value is Readonly<Record<string, unknown>> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		return false;
	}
	const prototype = Object.getPrototypeOf(value);
	return prototype === Object.prototype || prototype === null;
}

export function outputInvalid(path: string): never {
	throw new ReplicaCommandRuntimeError('REPLICA_COMMAND_PROTOCOL_INVALID', {
		cause: new TypeError(`invalid command output at ${path}`)
	});
}

export function compareCodeUnits(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

export function comparePropertyKeys(left: PropertyKey, right: PropertyKey): number {
	return compareCodeUnits(String(left), String(right));
}
