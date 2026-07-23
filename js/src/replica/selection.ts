import type {
	ReplicaEntitySelection,
	ReplicaObjectBranch,
	ReplicaObjectSelection,
	ReplicaRootSelection,
	ReplicaScalarSelection,
	ReplicaSelectionStorage
} from './types.js';

export type RuntimeScalarSelection = ReplicaScalarSelection & {
	readonly nullable: boolean;
};

export type RuntimeObjectSelection = {
	readonly typename: string;
	readonly storage: ReplicaSelectionStorage;
	readonly members: readonly RuntimeObjectMember[];
	readonly exactNullability: boolean;
	readonly revisionResponseKey?: string;
	readonly incarnationResponseKey?: string;
};

export type RuntimeObjectBranch = Omit<ReplicaObjectBranch, 'selection'> & {
	readonly selection: RuntimeObjectSelection;
	readonly expose?: boolean;
};

export type RuntimeObjectMember = RuntimeScalarSelection | RuntimeObjectBranch;

export type RuntimeRootSelection = Omit<ReplicaRootSelection, 'nullable' | 'selection'> & {
	readonly nullable: boolean;
	readonly selection: RuntimeObjectSelection;
};

export function runtimeRoot(selection: ReplicaRootSelection): RuntimeRootSelection {
	assertBranchShape(selection, 'root');
	if (
		isRecursiveObject(selection.selection) &&
		typeof selection.nullable !== 'boolean'
	) {
		throw new TypeError(
			`root ${selection.responseKey} must declare exact nullability`
		);
	}
	return Object.freeze({
		...selection,
		nullable: selection.nullable ?? true,
		selection: runtimeObject(selection.selection)
	});
}

export function embeddedRecordKey(
	artifactId: string,
	enclosingIndexKey: string,
	ordinal: number | undefined
): string {
	const slot = ordinal === undefined ? 'one' : `ordinal:${ordinal}`;
	return `embedded:${encodeURIComponent(artifactId)}:${encodeURIComponent(
		enclosingIndexKey
	)}:${slot}`;
}

function runtimeObject(
	selection: ReplicaEntitySelection | ReplicaObjectSelection
): RuntimeObjectSelection {
	if (isRecursiveObject(selection)) {
		assertName(selection.typename, 'selection typename');
		assertStorage(selection.storage);
		if (!Array.isArray(selection.members)) {
			throw new TypeError(`selection ${selection.typename} members must be an array`);
		}
		return Object.freeze({
			typename: selection.typename,
			storage: freezeStorage(selection.storage),
			exactNullability: true,
			members: Object.freeze(
				selection.members.map((member) => {
					if (member.kind === 'scalar') return runtimeScalar(member, false);
					if (member.kind !== 'branch') {
						throw new TypeError(
							`selection ${selection.typename} contains an unsupported member`
						);
					}
					assertBranchShape(member, `branch ${member.responseKey}`);
					if (
						member.semantic !== 'relationship' &&
						member.semantic !== 'aggregate' &&
						member.semantic !== 'aggregate_fields' &&
						member.semantic !== 'aggregate_nodes'
					) {
						throw new TypeError(
							`branch ${member.responseKey} has unsupported semantic ${String(
								member.semantic
							)}`
						);
					}
					if (typeof member.nullable !== 'boolean') {
						throw new TypeError(
							`branch ${member.responseKey} must declare exact nullability`
						);
					}
					return Object.freeze({
						...member,
						dependencies: freezeDependencies(member.dependencies, member.responseKey),
						selection: runtimeObject(member.selection)
					});
				})
			)
		});
	}

	assertName(selection.model.id, 'model id');
	if (
		!Array.isArray(selection.model.identityFields) ||
		selection.model.identityFields.length === 0
	) {
		throw new TypeError(
			`model ${selection.model.id} must declare at least one identity field`
		);
	}
	if (!Array.isArray(selection.fields)) {
		throw new TypeError(`model ${selection.model.id} fields must be an array`);
	}
	const members: RuntimeObjectMember[] = selection.fields.map((field) =>
		runtimeScalar(field, true)
	);
	for (const relationship of selection.relationships ?? []) {
		assertBranchShape(relationship, `relationship ${relationship.responseKey}`);
		members.push(
			Object.freeze({
				kind: 'branch' as const,
				semantic: 'relationship' as const,
				responseKey: relationship.responseKey,
				field: relationship.field,
				cardinality: relationship.cardinality,
				nullable: true,
				...(relationship.arguments === undefined
					? {}
					: { arguments: relationship.arguments }),
				dependencies: freezeDependencies(
					relationship.dependencies,
					relationship.responseKey
				),
				...(relationship.coverage === undefined
					? {}
					: { coverage: relationship.coverage }),
				...(relationship.expose === undefined
					? {}
					: { expose: relationship.expose }),
				selection: runtimeObject(relationship.selection)
			})
		);
	}
	return Object.freeze({
		typename: selection.model.id,
		storage: Object.freeze({
			kind: 'normalized' as const,
			model: selection.model.id,
			identityFields: Object.freeze([...selection.model.identityFields])
		}),
		exactNullability: false,
		members: Object.freeze(members),
		...(selection.revisionResponseKey === undefined
			? {}
			: { revisionResponseKey: selection.revisionResponseKey }),
		...(selection.incarnationResponseKey === undefined
			? {}
			: { incarnationResponseKey: selection.incarnationResponseKey })
	});
}

function runtimeScalar(
	selection: ReplicaScalarSelection,
	legacy: boolean
): RuntimeScalarSelection {
	if (selection.kind !== 'scalar') {
		throw new TypeError('object scalar member must have kind scalar');
	}
	assertName(selection.responseKey, 'scalar response key');
	assertName(selection.field, `scalar ${selection.responseKey} field`);
	if (!legacy) {
		assertName(selection.codec, `scalar ${selection.responseKey} codec`);
		if (typeof selection.nullable !== 'boolean') {
			throw new TypeError(
				`scalar ${selection.responseKey} must declare exact nullability`
			);
		}
	}
	return Object.freeze({
		...selection,
		nullable: selection.nullable ?? true
	});
}

function assertBranchShape(
	selection: Pick<
		ReplicaRootSelection | ReplicaObjectBranch,
		'responseKey' | 'field' | 'cardinality' | 'dependencies'
	>,
	description: string
): void {
	assertName(selection.responseKey, `${description} response key`);
	assertName(selection.field, `${description} field`);
	if (selection.cardinality !== 'one' && selection.cardinality !== 'many') {
		throw new TypeError(`${description} has unsupported cardinality`);
	}
	freezeDependencies(selection.dependencies, selection.responseKey);
}

function freezeDependencies(
	dependencies: readonly string[],
	responseKey: string
): readonly string[] {
	if (!Array.isArray(dependencies)) {
		throw new TypeError(`branch ${responseKey} dependencies must be an array`);
	}
	for (const dependency of dependencies) {
		assertName(dependency, `branch ${responseKey} dependency`);
	}
	return Object.freeze([...dependencies]);
}

function assertStorage(storage: ReplicaSelectionStorage): void {
	if (storage.kind === 'embedded') return;
	if (storage.kind !== 'normalized') {
		throw new TypeError('selection has unsupported storage');
	}
	assertName(storage.model, 'normalized model');
	if (!Array.isArray(storage.identityFields) || storage.identityFields.length === 0) {
		throw new TypeError(
			`normalized model ${storage.model} must declare at least one identity field`
		);
	}
	const unique = new Set<string>();
	for (const field of storage.identityFields) {
		assertName(field, `normalized model ${storage.model} identity field`);
		if (unique.has(field)) {
			throw new TypeError(
				`normalized model ${storage.model} contains duplicate identity field ${field}`
			);
		}
		unique.add(field);
	}
}

function freezeStorage(storage: ReplicaSelectionStorage): ReplicaSelectionStorage {
	if (storage.kind === 'embedded') return Object.freeze({ kind: 'embedded' as const });
	return Object.freeze({
		kind: 'normalized' as const,
		model: storage.model,
		identityFields: Object.freeze([...storage.identityFields])
	});
}

function isRecursiveObject(
	selection: ReplicaEntitySelection | ReplicaObjectSelection
): selection is ReplicaObjectSelection {
	return (
		Object.prototype.hasOwnProperty.call(selection, 'storage') &&
		Object.prototype.hasOwnProperty.call(selection, 'members')
	);
}

function assertName(value: unknown, description: string): asserts value is string {
	if (typeof value !== 'string' || value.length === 0) {
		throw new TypeError(`${description} must be a non-empty string`);
	}
}
