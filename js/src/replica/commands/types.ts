import type {
	DistributedCommandConsistency,
	DistributedTrustedPreset,
	DistributedTrustedPresetCodec
} from '../../protocol.js';
import type { ReplicaClientSurface, ReplicaValue } from '../types.js';

	/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export type ReplicaCommandScalarCodec = DistributedTrustedPresetCodec;

/** Static name/codec declaration emitted without a server-derived value. */
export type ReplicaTrustedPresetDescriptor = {
	readonly name: string;
	readonly codec: ReplicaCommandScalarCodec;
};

export type ReplicaCommandShape =
	| { readonly kind: 'none' }
	| {
			readonly kind: 'object';
			readonly definition: ReplicaCommandTypeDefinition;
	  };

export type ReplicaCommandTypeDefinition = {
	readonly name: string;
	readonly fields: readonly ReplicaCommandTypeField[];
};

export type ReplicaCommandTypeField = {
	readonly name: string;
	readonly typeName: string;
	readonly nullable: boolean;
	readonly list: boolean;
	readonly itemNullable: boolean;
	readonly codec?: ReplicaCommandScalarCodec;
	readonly nested?: ReplicaCommandTypeDefinition;
};

export type ReplicaCommandInputDefault = {
	readonly path: readonly string[];
	readonly generator: 'uuid_v7' | 'ulid';
};

export type ReplicaCommandInputDefaults = {
	readonly version: 1;
	readonly defaults: readonly ReplicaCommandInputDefault[];
};

export type ReplicaCommandEffectExpression =
	| { readonly kind: 'input'; readonly path: readonly string[] }
	| { readonly kind: 'trusted_preset'; readonly name: string }
	| { readonly kind: 'constant'; readonly value: ReplicaValue }
	| { readonly kind: 'null' };

export type ReplicaCommandEffectField = {
	readonly field: string;
	readonly value: ReplicaCommandEffectExpression;
};

export type ReplicaCommandEffectKey = {
	readonly fields: readonly ReplicaCommandEffectField[];
};

export type ReplicaCommandEffectRelationship = {
	readonly sourceModel: string;
	readonly field: string;
	readonly targetModel: string;
};

export type ReplicaCommandEffect =
	| {
			readonly kind: 'upsert' | 'patch';
			readonly model: string;
			readonly key: ReplicaCommandEffectKey;
			readonly fields: readonly ReplicaCommandEffectField[];
	  }
	| {
			readonly kind: 'delete';
			readonly model: string;
			readonly key: ReplicaCommandEffectKey;
	  }
	| {
			readonly kind: 'link' | 'unlink';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaCommandEffectKey;
			readonly target: ReplicaCommandEffectKey;
	  }
	| {
			readonly kind: 'invalidate_model';
			readonly model: string;
	  }
	| {
			readonly kind: 'invalidate_relationship';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaCommandEffectKey;
	  };

export type ReplicaCommandEffects = {
	readonly version: 1;
	readonly operations: readonly ReplicaCommandEffect[];
	readonly fallback: 'revalidate';
};

export type ReplicaCommandConfirmation = {
	readonly projector: string;
	readonly model: string;
	readonly key: ReplicaCommandEffectKey;
	readonly partition?: ReplicaCommandEffectExpression;
};

export type ReplicaCommandConfirmations =
	| {
			readonly version: 1;
			readonly kind: 'finite';
			readonly expected: readonly ReplicaCommandConfirmation[];
			readonly fallback: 'revalidate';
	  }
	| {
			readonly version: 1;
			readonly kind: 'unavailable';
			readonly expected: readonly [];
			readonly fallback: 'revalidate';
	  };

export type ReplicaCommandDirectProjection = {
	readonly topology: {
		readonly version: 1;
		readonly name: string;
		readonly digest: string;
	};
	readonly model: string;
	readonly identityFields: readonly string[];
	readonly partition?: ReplicaCommandEffectExpression;
	readonly changeEpoch: string;
};

/**
 * Conservative invalidation inventory emitted by the compiler.
 *
 * `dependencies` are the role-visible physical/model dependencies which task 9
 * may hand to the replica revalidator. `required` means the command cannot be
 * proven by its local effect/confirmation contract alone.
 */
export type ReplicaCommandRevalidationPlan = {
	readonly version: 1;
	readonly required: boolean;
	readonly dependencies: readonly string[];
	readonly models: readonly string[];
	readonly relationships: readonly ReplicaCommandEffectRelationship[];
};

/**
 * Framework-neutral executable command descriptor emitted by `dctl client`.
 *
 * The compiler has already validated the Rust-owned declaration. Runtime
 * validation remains fail-closed so a stale or hand-edited artifact cannot
 * create optimistic cache truth.
 */
export type ReplicaCommandArtifact<TInput = void, TOutput = unknown> = {
	readonly version: 1;
	readonly name: string;
	readonly mutationField: string;
	readonly document: string;
	readonly operationHash: string;
	readonly protocol: {
		readonly version: 2;
		readonly schemaHash: string;
		readonly protocolHash: string;
		readonly surface: ReplicaClientSurface;
		readonly operation: string;
		/** Exact scope-wide preset inventory for this generated client surface. */
		readonly trustedPresets: readonly ReplicaTrustedPresetDescriptor[];
	};
	readonly input: ReplicaCommandShape;
	readonly output: ReplicaCommandShape;
	readonly inputDefaults?: ReplicaCommandInputDefaults;
	readonly consistency: DistributedCommandConsistency;
	readonly effects: ReplicaCommandEffects;
	readonly confirmations?: ReplicaCommandConfirmations;
	readonly directProjection?: ReplicaCommandDirectProjection;
	readonly trustedPresets?: readonly ReplicaTrustedPresetDescriptor[];
	readonly revalidation: ReplicaCommandRevalidationPlan;
	/** Phantom slots preserve generated input/output types. */
	readonly __input?: TInput;
	readonly __output?: TOutput;
};

export type ReplicaPreparedEffectField = {
	readonly field: string;
	readonly value: ReplicaValue;
};

export type ReplicaPreparedEffectKey = {
	readonly fields: readonly ReplicaPreparedEffectField[];
};

export type ReplicaPreparedCommandEffect =
	| {
			readonly kind: 'upsert' | 'patch';
			readonly model: string;
			readonly key: ReplicaPreparedEffectKey;
			readonly fields: readonly ReplicaPreparedEffectField[];
	  }
	| {
			readonly kind: 'delete';
			readonly model: string;
			readonly key: ReplicaPreparedEffectKey;
	  }
	| {
			readonly kind: 'link' | 'unlink';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaPreparedEffectKey;
			readonly target: ReplicaPreparedEffectKey;
	  }
	| {
			readonly kind: 'invalidate_model';
			readonly model: string;
	  }
	| {
			readonly kind: 'invalidate_relationship';
			readonly relationship: ReplicaCommandEffectRelationship;
			readonly source: ReplicaPreparedEffectKey;
	  };

export type ReplicaPreparedConfirmation = {
	readonly projector: string;
	readonly model: string;
	readonly key: ReplicaPreparedEffectKey;
	readonly partition?: ReplicaValue;
};

export type ReplicaPreparedConfirmations =
	| {
			readonly kind: 'finite';
			readonly expected: readonly ReplicaPreparedConfirmation[];
	  }
	| { readonly kind: 'unavailable' };

export type DeepReadonly<T> = T extends (...args: never[]) => unknown
	? T
	: T extends readonly (infer TItem)[]
		? readonly DeepReadonly<TItem>[]
		: T extends object
			? { readonly [TKey in keyof T]: DeepReadonly<T[TKey]> }
			: T;

export type ReplicaCommandVariables<TInput> = [TInput] extends [void]
	? Readonly<{ commandId: string }>
	: Readonly<{ commandId: string; input: DeepReadonly<TInput> }>;

export type ReplicaPreparedCommand<TInput = void, TOutput = unknown> = {
	readonly name: string;
	readonly commandId: string;
	readonly consistency: DistributedCommandConsistency;
	readonly input: DeepReadonly<TInput>;
	readonly transport: {
		readonly mutationField: string;
		readonly document: string;
		readonly operationHash: string;
		readonly protocol: ReplicaCommandArtifact<TInput, TOutput>['protocol'];
		readonly variables: ReplicaCommandVariables<TInput>;
	};
	readonly optimistic: {
		readonly version: 1;
		readonly operations: readonly ReplicaPreparedCommandEffect[];
		readonly fallback: 'revalidate';
	};
	readonly confirmations?: ReplicaPreparedConfirmations;
	readonly directProjection?: {
		readonly topology: ReplicaCommandDirectProjection['topology'];
		readonly model: string;
		readonly identityFields: readonly string[];
		readonly partition?: ReplicaValue;
		readonly changeEpoch: string;
	};
	readonly revalidation: ReplicaCommandRevalidationPlan;
	/** Phantom output slot for generated wrappers. */
	readonly __output?: TOutput;
};

export type ReplicaCommandGenerators = {
	readonly uuidV7?: () => string;
	readonly ulid?: () => string;
};

export type PrepareReplicaCommandOptions = {
	readonly commandId?: string;
	readonly generators?: ReplicaCommandGenerators;
};

/**
 * Exact immutable command-local view over a server-derived scope inventory.
 *
 * The backing lookup is intentionally not exposed as a mutable Map.
 *
 * @internal
 */
export type ReplicaMatchedTrustedPresetInventory = Readonly<{
	descriptors: readonly ReplicaTrustedPresetDescriptor[];
	values: readonly DistributedTrustedPreset[];
	resolve(name: string): ReplicaValue;
}>;

export type ReplicaReceiptVerification =
	| {
			readonly kind: 'matched';
			readonly revalidate: boolean;
	  }
	| {
			readonly kind: 'deferred';
			readonly revalidate: boolean;
	  }
	| {
			readonly kind: 'revalidate';
			readonly revalidate: true;
			readonly reason: 'confirmation_unavailable';
	  };

export type ReplicaCommandContractErrorCode =
	| 'REPLICA_COMMAND_ARTIFACT_INVALID'
	| 'REPLICA_COMMAND_INPUT_INVALID'
	| 'REPLICA_COMMAND_RECEIPT_MISMATCH'
	| 'REPLICA_COMMAND_TRUSTED_PRESET_MISMATCH';

export type TrustedPresetReference = Readonly<{ name: string; path: string }>;
