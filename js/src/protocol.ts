/**
 * Versioned framework metadata carried in GraphQL's top-level
 * `extensions.distributed` response envelope.
 *
 * Domain result objects never contain these values. Hidden identities and
 * positions remain opaque strings: the JavaScript client compares or returns
 * them, but never parses them as numbers or reconstructs server scopes.
 */

/** The only Distributed GraphQL protocol version understood by this package. */
export const DISTRIBUTED_PROTOCOL_VERSION = 2 as const;

declare const opaqueDistributedString: unique symbol;
declare const distributedDecimalString: unique symbol;

/** Server-owned identity/token whose contents must not be interpreted by clients. */
export type DistributedOpaqueString = string & {
	readonly [opaqueDistributedString]: true;
};

/** Canonical unsigned u64 decimal carried without JavaScript numeric coercion. */
export type DistributedDecimalString = string & {
	readonly [distributedDecimalString]: true;
};

export type DistributedCommandState =
	| 'in_progress'
	| 'accepted'
	| 'accepted_pending_projection'
	| 'projected'
	| 'rejected'
	| 'projection_failed'
	| 'expired'
	| 'unknown';

export type DistributedCommandConsistency =
	| 'accepted'
	| 'fact'
	| 'projected';

/** One finite, server-resolved projection obligation for a command. */
export type DistributedProjectionExpectation = Readonly<
	Record<string, unknown> & {
		projection: string;
		model: string;
		scopeToken: DistributedOpaqueString;
	}
>;

/** Comparable record evidence within one exact opaque record scope. */
export type DistributedRecordRevision = Readonly<
	Record<string, unknown> & {
		path?: readonly string[];
		model: string;
		scopeToken: DistributedOpaqueString;
		incarnation: DistributedDecimalString;
		revision: DistributedDecimalString;
		tombstone: boolean;
	}
>;

/** Exact observation of one command causation in one projection obligation. */
export type DistributedProjectionObservation = Readonly<
	Record<string, unknown> & {
		causationId: DistributedOpaqueString;
		projection: string;
		model: string;
		scopeToken: DistributedOpaqueString;
	}
>;

/** Opaque resumable position for one named projection scope. */
export type DistributedLiveCursor = Readonly<
	Record<string, unknown> & {
		projection: string;
		position: DistributedDecimalString;
		token: DistributedOpaqueString;
	}
>;

/** Comparable checkpoint for one exact query-index projection member. */
export type DistributedIndexRevision = Readonly<
	Record<string, unknown> & {
		projection: string;
		scopeToken: DistributedOpaqueString;
		position: DistributedDecimalString;
		resume?: DistributedLiveCursor;
	}
>;

/** Record, index, and causation evidence for one exact operation instance. */
export type DistributedQuerySnapshot = Readonly<
	Record<string, unknown> & {
		scopeToken: DistributedOpaqueString;
		complete: boolean;
		records: readonly DistributedRecordRevision[];
		indexes: readonly DistributedIndexRevision[];
		observations: readonly DistributedProjectionObservation[];
	}
>;

/** Per-frame decision about live support, reset, and resumable cursors. */
export type DistributedLiveMetadata = Readonly<
	Record<string, unknown> & {
		supported: boolean;
		reset: boolean;
		cursors: readonly DistributedLiveCursor[];
	}
>;

/** Private GraphQL request extension consumed by generated live operations. */
export type DistributedLiveResumeExtensions = Readonly<{
	distributed: Readonly<{
		resume: Readonly<{
			cursors: readonly DistributedLiveCursor[];
		}>;
	}>;
}>;

/** Durable receipt/status metadata for one idempotent command identity. */
export type DistributedCommandMetadata = Readonly<
	Record<string, unknown> & {
		commandId: DistributedOpaqueString;
		causationId: DistributedOpaqueString;
		state: DistributedCommandState;
		consistency: DistributedCommandConsistency;
		expects: readonly DistributedProjectionExpectation[];
		/** Defaults to an empty array when omitted by the compact wire format. */
		observations: readonly DistributedProjectionObservation[];
		/** Defaults to an empty array when omitted by the compact wire format. */
		records: readonly DistributedRecordRevision[];
	}
>;

/** Canonical contents of top-level `extensions.distributed`. */
export type DistributedProtocolEnvelope = Readonly<
	Record<string, unknown> & {
		protocolVersion: typeof DISTRIBUTED_PROTOCOL_VERSION;
		schemaHash: string;
		cacheScope: DistributedOpaqueString;
		operation?: string;
		command?: DistributedCommandMetadata;
		snapshot?: DistributedQuerySnapshot;
		live?: DistributedLiveMetadata;
	}
>;

/** GraphQL top-level extensions with a validated Distributed envelope. */
export type GraphqlResponseExtensions = Readonly<
	Record<string, unknown> & {
		distributed?: DistributedProtocolEnvelope;
	}
>;

export type DistributedProtocolErrorCode =
	| 'DISTRIBUTED_PROTOCOL_INVALID'
	| 'DISTRIBUTED_PROTOCOL_VERSION_UNSUPPORTED';

/** Safe parse failure that reports structure, never hidden server values. */
export class DistributedProtocolError extends Error {
	readonly code: DistributedProtocolErrorCode;
	readonly path: string;

	constructor(code: DistributedProtocolErrorCode, path: string) {
		super(
			code === 'DISTRIBUTED_PROTOCOL_VERSION_UNSUPPORTED'
				? 'Unsupported Distributed GraphQL protocol version'
				: `Invalid Distributed GraphQL protocol envelope at ${path}`
		);
		this.name = 'DistributedProtocolError';
		this.code = code;
		this.path = path;
	}
}

const COMMAND_STATES = new Set<DistributedCommandState>([
	'in_progress',
	'accepted',
	'accepted_pending_projection',
	'projected',
	'rejected',
	'projection_failed',
	'expired',
	'unknown'
]);

const COMMAND_CONSISTENCIES = new Set<DistributedCommandConsistency>([
	'accepted',
	'fact',
	'projected'
]);

const MAX_PUBLIC_NAME_LENGTH = 512;
const MAX_OPAQUE_STRING_LENGTH = 16_384;
const MAX_EVIDENCE_ITEMS = 4_096;
const MAX_LIVE_RESUME_CURSORS = 64;
const MAX_PATH_SEGMENTS = 256;
const MAX_UNSIGNED_64 = '18446744073709551615';

/**
 * Parse GraphQL's optional top-level `extensions` object.
 *
 * Non-Distributed extension keys are preserved. When `distributed` is present,
 * its required version/scope/schema fields and known receipt/evidence fields are
 * validated. A malformed or incompatible envelope throws
 * {@link DistributedProtocolError}; transports convert that into a fail-closed
 * GraphQL result/error rather than exposing the accompanying data as trusted.
 */
export function parseGraphqlResponseExtensions(
	value: unknown
): GraphqlResponseExtensions | undefined {
	if (value === undefined) return undefined;
	const extensions = record(value, 'extensions');
	if (extensions.distributed === undefined) {
		return Object.freeze({ ...extensions });
	}

	return Object.freeze({
		...extensions,
		distributed: parseDistributedProtocolEnvelope(extensions.distributed)
	});
}

/** Parse one `extensions.distributed` value independently of a transport. */
export function parseDistributedProtocolEnvelope(
	value: unknown
): DistributedProtocolEnvelope {
	const envelope = record(value, 'extensions.distributed');
	if (envelope.protocolVersion !== DISTRIBUTED_PROTOCOL_VERSION) {
		if (
			typeof envelope.protocolVersion === 'number' &&
			Number.isInteger(envelope.protocolVersion)
		) {
			throw new DistributedProtocolError(
				'DISTRIBUTED_PROTOCOL_VERSION_UNSUPPORTED',
				'extensions.distributed.protocolVersion'
			);
		}
		invalid('extensions.distributed.protocolVersion');
	}

	const schemaHash = publicString(
		envelope.schemaHash,
		'extensions.distributed.schemaHash'
	);
	const cacheScope = opaqueString(
		envelope.cacheScope,
		'extensions.distributed.cacheScope'
	);
	const operation =
		envelope.operation === undefined
			? undefined
			: publicString(
					envelope.operation,
					'extensions.distributed.operation'
				);
	const command =
		envelope.command === undefined
			? undefined
			: parseCommand(envelope.command);
	const snapshot =
		envelope.snapshot === undefined
			? undefined
			: parseSnapshot(envelope.snapshot);
	const live =
		envelope.live === undefined
			? undefined
			: parseLive(envelope.live);

	return Object.freeze({
		...envelope,
		protocolVersion: DISTRIBUTED_PROTOCOL_VERSION,
		schemaHash,
		cacheScope,
		...(operation === undefined ? {} : { operation }),
		...(command === undefined ? {} : { command }),
		...(snapshot === undefined ? {} : { snapshot }),
		...(live === undefined ? {} : { live })
	}) as DistributedProtocolEnvelope;
}

/**
 * Build the private request extension for a generated live resume.
 *
 * The server remains authoritative and verifies every token. This client-side
 * check only prevents malformed, duplicate, or unbounded state from leaving
 * the replica.
 *
 * @internal
 */
export function distributedLiveResumeExtensions(
	value: readonly DistributedLiveCursor[]
): DistributedLiveResumeExtensions {
	if (
		!Array.isArray(value) ||
		value.length === 0 ||
		value.length > MAX_LIVE_RESUME_CURSORS
	) {
		invalid('request.extensions.distributed.resume.cursors');
	}
	const cursors = Object.freeze(
		value.map((cursor, index) =>
			parseLiveCursor(
				cursor,
				`request.extensions.distributed.resume.cursors[${index}]`
			)
		)
	);
	assertUnique(
		cursors.map((cursor) => cursor.projection),
		'request.extensions.distributed.resume.cursors'
	);
	return Object.freeze({
		distributed: Object.freeze({
			resume: Object.freeze({ cursors })
		})
	});
}

function parseCommand(value: unknown): DistributedCommandMetadata {
	const command = record(value, 'extensions.distributed.command');
	const state = enumString(
		command.state,
		COMMAND_STATES,
		'extensions.distributed.command.state'
	);
	const consistency = enumString(
		command.consistency,
		COMMAND_CONSISTENCIES,
		'extensions.distributed.command.consistency'
	);
	if (!Array.isArray(command.expects)) {
		invalid('extensions.distributed.command.expects');
	}
	if (command.expects.length > MAX_EVIDENCE_ITEMS) {
		invalid('extensions.distributed.command.expects');
	}

	const expects = command.expects.map((expectation, index) => {
		const path = `extensions.distributed.command.expects[${index}]`;
		const item = record(expectation, path);
		return Object.freeze({
			...item,
			projection: publicString(item.projection, `${path}.projection`),
			model: publicString(item.model, `${path}.model`),
			scopeToken: opaqueString(item.scopeToken, `${path}.scopeToken`)
		}) as DistributedProjectionExpectation;
	});
	const observations = parseOptionalEvidenceArray(
		command.observations,
		'extensions.distributed.command.observations',
		parseProjectionObservation
	);
	const records = parseOptionalEvidenceArray(
		command.records,
		'extensions.distributed.command.records',
		parseRecordRevision
	);
	const commandId = opaqueString(
		command.commandId,
		'extensions.distributed.command.commandId'
	);
	const causationId = opaqueString(
		command.causationId,
		'extensions.distributed.command.causationId'
	);
	const expectationKeys = new Set(expects.map(projectionExpectationKey));
	const seenObservations = new Set<string>();
	for (const observation of observations) {
		const key = projectionObservationKey(observation);
		if (
			observation.causationId !== causationId ||
			!expectationKeys.has(projectionExpectationKey(observation)) ||
			seenObservations.has(key)
		) {
			invalid('extensions.distributed.command.observations');
		}
		seenObservations.add(key);
	}

	return Object.freeze({
		...command,
		commandId,
		causationId,
		state,
		consistency,
		expects: Object.freeze(expects),
		observations,
		records
	}) as DistributedCommandMetadata;
}

function parseSnapshot(value: unknown): DistributedQuerySnapshot {
	const path = 'extensions.distributed.snapshot';
	const snapshot = record(value, path);
	if (typeof snapshot.complete !== 'boolean') invalid(`${path}.complete`);
	const records = parseRequiredEvidenceArray(
		snapshot.records,
		`${path}.records`,
		parseRecordRevision
	);
	const indexes = parseRequiredEvidenceArray(
		snapshot.indexes,
		`${path}.indexes`,
		parseIndexRevision
	);
	const observations = parseRequiredEvidenceArray(
		snapshot.observations,
		`${path}.observations`,
		parseProjectionObservation
	);
	assertUnique(
		indexes.map((index) => index.projection),
		`${path}.indexes`
	);
	if (
		indexes.filter((index) => index.resume !== undefined).length >
		MAX_LIVE_RESUME_CURSORS
	) {
		invalid(`${path}.indexes`);
	}

	return Object.freeze({
		...snapshot,
		scopeToken: opaqueString(snapshot.scopeToken, `${path}.scopeToken`),
		complete: snapshot.complete,
		records,
		indexes,
		observations
	}) as DistributedQuerySnapshot;
}

function parseLive(value: unknown): DistributedLiveMetadata {
	const path = 'extensions.distributed.live';
	const live = record(value, path);
	if (typeof live.supported !== 'boolean') invalid(`${path}.supported`);
	if (typeof live.reset !== 'boolean') invalid(`${path}.reset`);
	if (
		!Array.isArray(live.cursors) ||
		live.cursors.length > MAX_LIVE_RESUME_CURSORS
	) {
		invalid(`${path}.cursors`);
	}
	const cursors = parseRequiredEvidenceArray(
		live.cursors,
		`${path}.cursors`,
		parseLiveCursor
	);
	assertUnique(
		cursors.map((cursor) => cursor.projection),
		`${path}.cursors`
	);
	if (!live.supported && (!live.reset || cursors.length !== 0)) {
		invalid(path);
	}
	return Object.freeze({
		...live,
		supported: live.supported,
		reset: live.reset,
		cursors
	}) as DistributedLiveMetadata;
}

function parseRecordRevision(
	value: unknown,
	path: string
): DistributedRecordRevision {
	const item = record(value, path);
	let responsePath: readonly string[] | undefined;
	if (item.path !== undefined) {
		if (
			!Array.isArray(item.path) ||
			item.path.length === 0 ||
			item.path.length > MAX_PATH_SEGMENTS
		) {
			invalid(`${path}.path`);
		}
		responsePath = Object.freeze(
			item.path.map((segment, index) =>
				publicString(segment, `${path}.path[${index}]`)
			)
		);
	}
	if (typeof item.tombstone !== 'boolean') invalid(`${path}.tombstone`);
	return Object.freeze({
		...item,
		...(responsePath === undefined ? {} : { path: responsePath }),
		model: publicString(item.model, `${path}.model`),
		scopeToken: opaqueString(item.scopeToken, `${path}.scopeToken`),
		incarnation: canonicalDecimal(item.incarnation, `${path}.incarnation`),
		revision: canonicalDecimal(item.revision, `${path}.revision`),
		tombstone: item.tombstone
	}) as DistributedRecordRevision;
}

function parseProjectionObservation(
	value: unknown,
	path: string
): DistributedProjectionObservation {
	const item = record(value, path);
	return Object.freeze({
		...item,
		causationId: opaqueString(item.causationId, `${path}.causationId`),
		projection: publicString(item.projection, `${path}.projection`),
		model: publicString(item.model, `${path}.model`),
		scopeToken: opaqueString(item.scopeToken, `${path}.scopeToken`)
	}) as DistributedProjectionObservation;
}

function parseIndexRevision(
	value: unknown,
	path: string
): DistributedIndexRevision {
	const item = record(value, path);
	const projection = publicString(item.projection, `${path}.projection`);
	const position = canonicalDecimal(item.position, `${path}.position`);
	const resume =
		item.resume === undefined
			? undefined
			: parseLiveCursor(item.resume, `${path}.resume`);
	if (
		resume !== undefined &&
		(resume.projection !== projection || resume.position !== position)
	) {
		invalid(`${path}.resume`);
	}
	return Object.freeze({
		...item,
		projection,
		scopeToken: opaqueString(item.scopeToken, `${path}.scopeToken`),
		position,
		...(resume === undefined ? {} : { resume })
	}) as DistributedIndexRevision;
}

function parseLiveCursor(value: unknown, path: string): DistributedLiveCursor {
	const item = record(value, path);
	return Object.freeze({
		...item,
		projection: publicString(item.projection, `${path}.projection`),
		position: canonicalDecimal(item.position, `${path}.position`),
		token: opaqueString(item.token, `${path}.token`)
	}) as DistributedLiveCursor;
}

function parseRequiredEvidenceArray<T>(
	value: unknown,
	path: string,
	parse: (value: unknown, path: string) => T
): readonly T[] {
	if (!Array.isArray(value) || value.length > MAX_EVIDENCE_ITEMS) invalid(path);
	return Object.freeze(value.map((item, index) => parse(item, `${path}[${index}]`)));
}

function parseOptionalEvidenceArray<T>(
	value: unknown,
	path: string,
	parse: (value: unknown, path: string) => T
): readonly T[] {
	return value === undefined
		? Object.freeze([])
		: parseRequiredEvidenceArray(value, path, parse);
}

function projectionExpectationKey(
	value: Pick<
		DistributedProjectionExpectation | DistributedProjectionObservation,
		'projection' | 'model' | 'scopeToken'
	>
): string {
	return JSON.stringify([value.projection, value.model, value.scopeToken]);
}

function projectionObservationKey(value: DistributedProjectionObservation): string {
	return JSON.stringify([
		value.causationId,
		value.projection,
		value.model,
		value.scopeToken
	]);
}

function assertUnique(values: readonly string[], path: string): void {
	if (new Set(values).size !== values.length) invalid(path);
}

function record(value: unknown, path: string): Record<string, unknown> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		invalid(path);
	}
	const prototype = Object.getPrototypeOf(value);
	if (prototype !== Object.prototype && prototype !== null) invalid(path);
	return value as Record<string, unknown>;
}

function publicString(value: unknown, path: string): string {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value.length > MAX_PUBLIC_NAME_LENGTH
	) {
		invalid(path);
	}
	return value;
}

function opaqueString(value: unknown, path: string): DistributedOpaqueString {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value.length > MAX_OPAQUE_STRING_LENGTH
	) {
		invalid(path);
	}
	return value as DistributedOpaqueString;
}

function canonicalDecimal(
	value: unknown,
	path: string
): DistributedDecimalString {
	if (
		typeof value !== 'string' ||
		!/^(0|[1-9][0-9]*)$/.test(value) ||
		value.length > MAX_UNSIGNED_64.length ||
		(value.length === MAX_UNSIGNED_64.length && value > MAX_UNSIGNED_64)
	) {
		invalid(path);
	}
	return value as DistributedDecimalString;
}

/**
 * Compare two already-validated protocol decimals without converting either
 * through JavaScript's numeric types.
 */
export function compareDistributedDecimal(
	left: DistributedDecimalString,
	right: DistributedDecimalString
): -1 | 0 | 1 {
	if (left.length !== right.length) return left.length < right.length ? -1 : 1;
	return left === right ? 0 : left < right ? -1 : 1;
}

function enumString<T extends string>(
	value: unknown,
	values: ReadonlySet<T>,
	path: string
): T {
	if (typeof value !== 'string' || !values.has(value as T)) invalid(path);
	return value as T;
}

function invalid(path: string): never {
	throw new DistributedProtocolError('DISTRIBUTED_PROTOCOL_INVALID', path);
}
