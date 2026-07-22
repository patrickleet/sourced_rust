import type { GqlError, GraphqlVariables } from '../types.js';

export type ReplicaRevision = number | string | bigint;
export type ReplicaValue =
	| null
	| boolean
	| number
	| string
	| readonly ReplicaValue[]
	| { readonly [key: string]: ReplicaValue };

export type ReplicaIndexCoverage =
	| { readonly kind: 'complete' }
	| { readonly kind: 'unknown' }
	| {
			readonly kind: 'offset';
			readonly offset: number;
			readonly limit?: number;
			readonly returned?: number;
			readonly hasNext?: boolean;
	  }
	| {
			readonly kind: 'cursor';
			readonly after?: ReplicaValue;
			readonly before?: ReplicaValue;
			readonly first?: number;
			readonly last?: number;
			readonly start?: ReplicaValue;
			readonly end?: ReplicaValue;
			readonly hasNext?: boolean;
			readonly hasPrevious?: boolean;
	  };

/** Minimal model identity emitted by a generated operation artifact. */
export type ReplicaModelArtifact = {
	readonly id: string;
	readonly identityFields: readonly string[];
};

export type ReplicaLiteralValue = {
	readonly kind: 'literal';
	readonly value: ReplicaValue;
};

export type ReplicaVariableValue = {
	readonly kind: 'variable';
	readonly name: string;
};

export type ReplicaArgumentValue = ReplicaLiteralValue | ReplicaVariableValue;
export type ReplicaArgumentsArtifact = Readonly<Record<string, ReplicaArgumentValue>>;

export type ReplicaCoverageArtifact =
	| { readonly kind: 'complete' }
	| {
			readonly kind: 'offset';
			readonly offsetArgument?: string;
			readonly limitArgument?: string;
	  }
	| {
			readonly kind: 'cursor';
			readonly afterArgument?: string;
			readonly beforeArgument?: string;
			readonly firstArgument?: string;
			readonly lastArgument?: string;
	  };

export type ReplicaScalarSelection = {
	readonly kind: 'scalar';
	/** GraphQL response key after aliases are applied. */
	readonly responseKey: string;
	/** Canonical schema/read-model field name. */
	readonly field: string;
	/** Wire-only identity/revision fields set this false. Defaults to true. */
	readonly expose?: boolean;
};

export type ReplicaEntitySelection = {
	readonly model: ReplicaModelArtifact;
	readonly fields: readonly ReplicaScalarSelection[];
	readonly relationships?: readonly ReplicaRelationshipSelection[];
	/** Optional injected row revision field; otherwise the envelope revision wins. */
	readonly revisionResponseKey?: string;
	/** Optional injected incarnation field; otherwise the cache derives it. */
	readonly incarnationResponseKey?: string;
};

export type ReplicaRelationshipSelection = {
	readonly kind: 'relationship';
	readonly responseKey: string;
	readonly field: string;
	readonly cardinality: 'one' | 'many';
	readonly arguments?: ReplicaArgumentsArtifact;
	readonly dependencies: readonly string[];
	readonly coverage?: ReplicaCoverageArtifact;
	readonly selection: ReplicaEntitySelection;
	readonly expose?: boolean;
};

export type ReplicaRootSelection = {
	readonly responseKey: string;
	readonly field: string;
	readonly cardinality: 'one' | 'many';
	readonly arguments?: ReplicaArgumentsArtifact;
	readonly dependencies: readonly string[];
	readonly coverage?: ReplicaCoverageArtifact;
	readonly selection: ReplicaEntitySelection;
	readonly expose?: boolean;
};

/**
 * Narrow runtime contract expected from generated code.
 *
 * It deliberately does not mirror the Rust client-manifest JSON. The compiler
 * owns lowering the manifest and GraphQL AST into this executable descriptor.
 */
export type ReplicaOperationArtifact<
	TData = Record<string, unknown>,
	TVariables extends GraphqlVariables = GraphqlVariables
> = {
	readonly id: string;
	readonly document: string;
	readonly roots: readonly ReplicaRootSelection[];
	readonly live?: {
		readonly id: string;
		readonly document: string;
	};
	/** Phantom slots preserve generated result/variables types. */
	readonly __result?: TData;
	readonly __variables?: TVariables;
};

export type ReplicaWriteSource = 'network' | 'live' | 'ssr' | 'restore' | 'projected';

export type ReplicaResultEnvelope<TData = unknown> = {
	readonly data?: TData | null;
	readonly errors?: readonly GqlError[];
	readonly revision: ReplicaRevision;
};

export type ReplicaTransportRequest<
	TData = Record<string, unknown>,
	TVariables extends GraphqlVariables = GraphqlVariables
> = {
	readonly operation: 'query' | 'live';
	readonly operationId: string;
	readonly document: string;
	readonly variables: TVariables;
	readonly artifact: ReplicaOperationArtifact<TData, TVariables>;
};

export type ReplicaLiveObserver<TData = unknown> = {
	next(result: ReplicaResultEnvelope<TData>): void;
	error(error: unknown): void;
};

export type ReplicaTransport = {
	fetch<
		TData = Record<string, unknown>,
		TVariables extends GraphqlVariables = GraphqlVariables
	>(
		request: ReplicaTransportRequest<TData, TVariables>
	): Promise<ReplicaResultEnvelope<TData>>;
	subscribe?<
		TData = Record<string, unknown>,
		TVariables extends GraphqlVariables = GraphqlVariables
	>(
		request: ReplicaTransportRequest<TData, TVariables>,
		observer: ReplicaLiveObserver<TData>
	): () => void;
};

export type ReplicaStatus = 'loading' | 'ready' | 'stale' | 'error';
export type ReplicaLiveState = 'off' | 'connecting' | 'active' | 'error';

/** The exact runtime shape while some selected cache fields are still absent. */
export type ReplicaSparse<T> = T extends readonly (infer TItem)[]
	? readonly ReplicaSparse<TItem>[]
	: T extends object
		? { readonly [K in keyof T]?: ReplicaSparse<T[K]> }
		: T;

type ReplicaSnapshotState = {
	readonly fetching: boolean;
	readonly errors: readonly GqlError[];
	readonly live: ReplicaLiveState;
};

/**
 * A complete snapshot carries the generated result type. Loading, stale, and
 * partial-error snapshots accurately expose a deep sparse result instead.
 */
export type ReplicaSnapshot<TData> = ReplicaSnapshotState &
	(
		| {
				readonly data: TData;
				readonly status: 'ready' | 'error';
				readonly stale: false;
				readonly complete: true;
		  }
		| {
				readonly data: ReplicaSparse<TData>;
				readonly status: 'loading';
				readonly stale: false;
				readonly complete: false;
		  }
		| {
				readonly data: ReplicaSparse<TData>;
				readonly status: 'stale';
				readonly stale: true;
				readonly complete: false;
		  }
		| {
				readonly data: ReplicaSparse<TData>;
				readonly status: 'error';
				readonly stale: boolean;
				readonly complete: false;
		  }
	);

export type ReplicaWatch<TData> = {
	get(): ReplicaSnapshot<TData>;
	subscribe(listener: (snapshot: ReplicaSnapshot<TData>) => void): () => void;
	refresh(): Promise<void>;
	destroy(): void;
};

export type WatchReplicaOptions = {
	readonly live?: boolean;
};

export type DistributedReplicaOptions = {
	readonly transport?: ReplicaTransport;
	readonly onObserverError?: (error: AggregateError) => void;
};

export type ReplicaRecordInspection = {
	readonly key: string;
	readonly revision: string;
	readonly incarnation: string;
	readonly presentFields: readonly string[];
};

export type ReplicaIndexInspection = {
	readonly key: string;
	readonly revision: string;
	readonly staleRevision?: string;
	readonly records: readonly string[];
	readonly complete: boolean;
	readonly field: string;
	readonly parent?: string;
	readonly arguments: Readonly<Record<string, ReplicaValue>>;
	readonly coverage: ReplicaIndexCoverage;
	readonly dependencies: readonly string[];
	readonly staleReason?: string;
	readonly nullValue: boolean;
};

export type ReplicaIdentity = ReplicaValue | readonly ReplicaValue[];

export type ReplicaIndexTarget = {
	readonly parent?: string;
	readonly field: string;
	readonly arguments?: Readonly<Record<string, ReplicaValue>>;
	readonly coverage?: ReplicaIndexCoverage;
	readonly dependencies?: readonly string[];
	readonly complete?: boolean;
	readonly staleReason?: string;
	readonly nullValue?: boolean;
};

export type ReplicaRecordPatch = {
	readonly fields?: Readonly<Record<string, ReplicaValue>>;
	readonly links?: Readonly<Record<string, string | readonly string[] | null>>;
};

export interface ReplicaOptimisticWriter {
	writeRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity,
		patch: ReplicaRecordPatch
	): void;
	tombstoneRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void;
	writeIndex(target: ReplicaIndexTarget, records: readonly string[]): void;
	deleteIndex(target: ReplicaIndexTarget): void;
}

export interface ReplicaBaseWriter {
	writeRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity,
		revision: ReplicaRevision,
		patch: ReplicaRecordPatch & { readonly incarnation?: ReplicaRevision }
	): boolean;
	tombstoneRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity,
		revision: ReplicaRevision
	): boolean;
	writeIndex(
		target: ReplicaIndexTarget,
		records: readonly string[],
		revision: ReplicaRevision
	): boolean;
	deleteIndex(target: ReplicaIndexTarget, revision: ReplicaRevision): boolean;
}

export interface DistributedReplica {
	read<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables
	): ReplicaSnapshot<TData>;
	watch<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		options?: WatchReplicaOptions
	): ReplicaWatch<TData>;
	writeResult<TData, TVariables extends GraphqlVariables>(
		artifact: ReplicaOperationArtifact<TData, TVariables>,
		variables: TVariables,
		envelope: ReplicaResultEnvelope<TData>,
		source: ReplicaWriteSource
	): void;
	createOptimisticLayer(
		id: string,
		update: (writer: ReplicaOptimisticWriter) => void
	): void;
	markOptimisticLayerAccepted(id: string): boolean;
	confirmOptimisticLayer<T>(
		id: string,
		update: (writer: ReplicaBaseWriter) => T
	): T;
	rejectOptimisticLayer(id: string): boolean;
	tombstoneRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity,
		revision: ReplicaRevision
	): boolean;
	markIndexStale(target: ReplicaIndexTarget, reason: string): boolean;
	retainRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void;
	releaseRecord(model: ReplicaModelArtifact, identity: ReplicaIdentity): void;
	gc(): readonly string[];
	inspectRecord(
		model: ReplicaModelArtifact,
		identity: ReplicaIdentity
	): ReplicaRecordInspection | undefined;
	inspectIndex(target: ReplicaIndexTarget): ReplicaIndexInspection | undefined;
}
