export class CacheRevisionConflictError extends Error {
	readonly dependency: string;
	readonly revision: string;

	constructor(dependency: string, revision: bigint) {
		super(`conflicting cache values at revision ${revision} for ${dependency}`);
		this.name = 'CacheRevisionConflictError';
		this.dependency = dependency;
		this.revision = revision.toString(10);
	}
}

export class OptimisticLayerNotFoundError extends Error {
	readonly layerId: string;

	constructor(layerId: string) {
		super(`optimistic layer not found: ${layerId}`);
		this.name = 'OptimisticLayerNotFoundError';
		this.layerId = layerId;
	}
}
