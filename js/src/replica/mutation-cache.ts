/**
 * Role-safe cache lowering for event-independent mutation IR (v1).
 *
 * Mirrors `distributed::mutation::cache::lower_mutation_cache`. Mutation IR is
 * the semantic source; this module produces atomic optimistic-layer effects
 * without a second browser CRUD interpreter.
 */

export type MutationCacheVisibility = {
	readonly authorized: boolean;
	readonly hasBaseRecord: boolean;
	readonly relationshipCovered: boolean;
};

export type MutationTarget = {
	readonly model: string;
	readonly storage: string;
};

export type MutationFieldAssignment = {
	readonly kind: "set" | "unset" | "unknown";
	readonly expression?: unknown;
};

export type MutationField = {
	readonly name: string;
	readonly assignment: MutationFieldAssignment;
};

export type MutationOperation = {
	readonly kind: string;
	readonly target: MutationTarget;
	readonly fields?: readonly MutationField[];
	readonly invalidations?: readonly unknown[];
};

export type MutationProgram = {
	readonly ir_version: number;
	readonly operations: readonly MutationOperation[];
	readonly name?: string;
};

export type MutationCacheEffect =
	| {
			readonly kind: "upsert";
			readonly target: MutationTarget;
			readonly fields: readonly string[];
	  }
	| {
			readonly kind: "patch";
			readonly target: MutationTarget;
			readonly fields: readonly string[];
	  }
	| {
			readonly kind: "delete";
			readonly target: MutationTarget;
	  }
	| {
			readonly kind: "invalidate";
			readonly invalidations: readonly unknown[];
	  };

export type MutationCacheProgram = {
	readonly effects: readonly MutationCacheEffect[];
};

export const MUTATION_CACHE_VISIBILITY_FULL: MutationCacheVisibility = Object.freeze({
	authorized: true,
	hasBaseRecord: true,
	relationshipCovered: true,
});

export const MUTATION_CACHE_VISIBILITY_UNAUTHORIZED: MutationCacheVisibility = Object.freeze({
	authorized: false,
	hasBaseRecord: true,
	relationshipCovered: true,
});

/**
 * Lower a canonical mutation program to role-safe cache effects.
 *
 * Partial patches never create records. Unauthorized or unprovable operations
 * fail closed to the narrowest model invalidation.
 */
export function lowerMutationCache(
	program: MutationProgram,
	visibility: MutationCacheVisibility = MUTATION_CACHE_VISIBILITY_FULL,
): MutationCacheProgram {
	const effects = (program.operations ?? []).map((operation) =>
		lowerOperation(operation, visibility),
	);
	return Object.freeze({ effects: Object.freeze(effects) });
}

function lowerOperation(
	operation: MutationOperation,
	visibility: MutationCacheVisibility,
): MutationCacheEffect {
	if (!visibility.authorized) {
		return invalidateModel(operation);
	}
	switch (operation.kind) {
		case "insert":
		case "upsert":
		case "recreate":
		case "insert_related":
		case "upsert_related": {
			const fields = concreteFieldNames(operation);
			if (fields.length === 0) {
				return invalidateModel(operation);
			}
			return Object.freeze({
				kind: "upsert",
				target: operation.target,
				fields: Object.freeze(fields),
			});
		}
		case "patch":
		case "upsert_patch": {
			if (!visibility.hasBaseRecord) {
				return invalidateModel(operation);
			}
			const fields = concreteFieldNames(operation);
			if (fields.length === 0) {
				return invalidateModel(operation);
			}
			return Object.freeze({
				kind: "patch",
				target: operation.target,
				fields: Object.freeze(fields),
			});
		}
		case "delete":
			return Object.freeze({
				kind: "delete",
				target: operation.target,
			});
		case "invalidate":
			return Object.freeze({
				kind: "invalidate",
				invalidations: Object.freeze(
					operation.invalidations?.length
						? [...operation.invalidations]
						: [{ kind: "model", model: operation.target.model }],
				),
			});
		default:
			throw new Error(`unsupported mutation kind: ${operation.kind}`);
	}
}

function concreteFieldNames(operation: MutationOperation): string[] {
	return (operation.fields ?? [])
		.filter(
			(field) =>
				field.assignment?.kind === "set" || field.assignment?.kind === "unset",
		)
		.map((field) => field.name);
}

function invalidateModel(operation: MutationOperation): MutationCacheEffect {
	return Object.freeze({
		kind: "invalidate",
		invalidations: Object.freeze([
			{ kind: "model", model: operation.target.model },
		]),
	});
}
