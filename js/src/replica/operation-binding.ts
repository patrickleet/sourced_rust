import { isDistributedTrustedPresetCodec } from '../protocol.js';
import type { GraphqlVariables } from '../types.js';
import type {
	ReplicaClientSurface,
	ReplicaOperationArtifact,
	ReplicaSurfaceTrustedPresetDescriptor
} from './types.js';

export type ValidatedReplicaOperationBinding = {
	readonly version: 2;
	readonly schemaHash: string;
	readonly operation: string;
	readonly surfaceIdentity: string;
	readonly trustedPresets: readonly ReplicaSurfaceTrustedPresetDescriptor[];
};

/**
 * Validate the exact compiler-owned protocol identity before an operation may
 * acquire a cache key, register a maintenance plan, or reach a transport.
 */
export function validateReplicaOperationBinding<
	TData,
	TVariables extends GraphqlVariables
>(
	artifact: ReplicaOperationArtifact<TData, TVariables>
): ValidatedReplicaOperationBinding {
	if (typeof artifact.id !== 'string' || artifact.id.length === 0) {
		throw new TypeError('replica artifact id must be a non-empty string');
	}
	const binding = artifact.protocol;
	if (
		binding === undefined ||
		binding.version !== 2 ||
		typeof binding.schemaHash !== 'string' ||
		binding.schemaHash.length === 0 ||
		typeof binding.operation !== 'string' ||
		binding.operation.length === 0 ||
		binding.operation !== artifact.id
	) {
		throw new TypeError('replica artifact protocol binding is invalid');
	}
	if (artifact.variableCodec === undefined) {
		throw new TypeError('protocol-v2 replica artifact requires variableCodec');
	}
	return Object.freeze({
		version: binding.version,
		schemaHash: binding.schemaHash,
		operation: binding.operation,
		surfaceIdentity: validateReplicaSurfaceIdentity(binding.surface),
		trustedPresets: canonicalTrustedPresetDescriptors(
			binding.trustedPresets
		)
	});
}

function validateReplicaSurfaceIdentity(value: ReplicaClientSurface): string {
	if (
		value === null ||
		typeof value !== 'object' ||
		typeof value.name !== 'string' ||
		value.name.length === 0
	) {
		throw new TypeError('replica artifact client surface is invalid');
	}
	if (value.kind === 'role') {
		return JSON.stringify(['role', value.name]);
	}
	if (
		value.kind !== 'application' ||
		!Array.isArray(value.roles) ||
		value.roles.length === 0 ||
		value.roles.some(
			(role) => typeof role !== 'string' || role.length === 0
		) ||
		new Set(value.roles).size !== value.roles.length ||
		[...value.roles].sort().some((role, index) => role !== value.roles[index])
	) {
		throw new TypeError('replica artifact client surface is invalid');
	}
	return JSON.stringify(['application', value.name, value.roles]);
}

function canonicalTrustedPresetDescriptors(
	value: unknown
): readonly ReplicaSurfaceTrustedPresetDescriptor[] {
	if (!Array.isArray(value)) {
		throw new TypeError('replica artifact trusted preset contract is invalid');
	}
	const names = new Set<string>();
	return Object.freeze(
		value
			.map((candidate) => {
				if (
					candidate === null ||
					typeof candidate !== 'object' ||
					typeof candidate.name !== 'string' ||
					candidate.name.length === 0 ||
					candidate.name.length > 128 ||
					candidate.name.trim() !== candidate.name ||
					/[\u0000-\u001f\u007f-\u009f]/.test(candidate.name) ||
					names.has(candidate.name) ||
					!isDistributedTrustedPresetCodec(candidate.codec)
				) {
					throw new TypeError(
						'replica artifact trusted preset contract is invalid'
					);
				}
				names.add(candidate.name);
				return Object.freeze({
					name: candidate.name,
					codec: candidate.codec
				});
			})
			.sort(({ name: left }, { name: right }) =>
				left < right ? -1 : left > right ? 1 : 0
			)
	);
}
