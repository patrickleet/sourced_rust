import type {
	CommandProjectionMetadata,
	ProjectionDelta
} from './types.js';

const encoder = new TextEncoder();

/** Canonical bytes use the exact field order of the frozen Rust serde structs. */
export function canonicalProjectionDelta(delta: ProjectionDelta): string {
	return JSON.stringify({
		wire_version: delta.wire_version,
		identity: delta.identity,
		projections: delta.projections,
		occurrences: delta.occurrences,
		operations: delta.operations,
		...(delta.recoveries.length === 0 ? {} : { recoveries: delta.recoveries })
	});
}

export function canonicalCommandProjectionMetadata(
	metadata: CommandProjectionMetadata
): string {
	return JSON.stringify({
		wireVersion: metadata.wireVersion,
		issuedAtUnixMs: metadata.issuedAtUnixMs,
		expiresAtUnixMs: metadata.expiresAtUnixMs,
		delta: JSON.parse(canonicalProjectionDelta(metadata.delta)) as unknown,
		obligations: metadata.obligations,
		revalidate: metadata.revalidate
	});
}

export function projectionDeltaByteLength(delta: ProjectionDelta): number {
	return encoder.encode(canonicalProjectionDelta(delta)).byteLength;
}

export function commandProjectionMetadataByteLength(
	metadata: CommandProjectionMetadata
): number {
	return encoder.encode(canonicalCommandProjectionMetadata(metadata)).byteLength;
}
