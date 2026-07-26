import type { GraphqlVariables } from '../../types.js';
import type {
	ReplicaCommandArtifact,
	ReplicaCommandEffectExpression
} from '../commands.js';
import type {
	ReplicaObjectSelection,
	ReplicaOperationArtifact,
	ReplicaRootSelection
} from '../types.js';
import type {
	ReplicaArtifactSourceLocation,
	ReplicaCommandArtifactInspection,
	ReplicaCommandEffectInspection,
	ReplicaOperationArtifactInspection,
	ReplicaOperationIndexInspection,
	ReplicaOperationInjectedFieldInspection
} from './types.js';

export function inspectReplicaOperationArtifact<
	TData,
	TVariables extends GraphqlVariables
>(
	artifact: ReplicaOperationArtifact<TData, TVariables>
): ReplicaOperationArtifactInspection {
	const injected: ReplicaOperationInjectedFieldInspection[] = [];
	const dependencies = new Set<string>();
	const indexes: ReplicaOperationIndexInspection[] = [];

	for (const root of artifact.roots) {
		const path = root.responseKey;
		inspectRoot(root, path, injected, dependencies, indexes);
	}

	const source = safeArtifactSource(artifact.source);
	return Object.freeze({
		kind: 'operation' as const,
		id: artifact.id,
		...(source === undefined
			? {}
			: {
					source: Object.freeze({
						path: source.path,
						line: source.line,
						column: source.column
					})
				}),
		rootFields: Object.freeze(artifact.roots.map((root) => root.field)),
		injectedFields: Object.freeze(injected),
		dependencies: Object.freeze([...dependencies].sort()),
		indexes: Object.freeze(indexes),
		...(artifact.live === undefined
			? {}
			: { live: Object.freeze({ operation: artifact.live.id }) })
	});
}

export function inspectReplicaCommandArtifact<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>
): ReplicaCommandArtifactInspection {
	const effects = artifact.effects.operations.map((effect) => {
		const models = new Set<string>();
		const fields = new Set<string>();
		const sources = new Set<ReplicaCommandEffectInspection['valueSources'][number]>();
		if ('model' in effect) models.add(effect.model);
		if ('relationship' in effect) {
			models.add(effect.relationship.sourceModel);
			models.add(effect.relationship.targetModel);
			fields.add(effect.relationship.field);
		}
		if ('key' in effect) {
			for (const field of effect.key.fields) {
				fields.add(field.field);
				sources.add(expressionSource(field.value));
			}
		}
		if ('source' in effect) {
			for (const field of effect.source.fields) {
				fields.add(field.field);
				sources.add(expressionSource(field.value));
			}
		}
		if ('target' in effect) {
			for (const field of effect.target.fields) {
				fields.add(field.field);
				sources.add(expressionSource(field.value));
			}
		}
		if ('fields' in effect) {
			for (const field of effect.fields) {
				fields.add(field.field);
				sources.add(expressionSource(field.value));
			}
		}
		return Object.freeze({
			kind: effect.kind,
			models: Object.freeze([...models].sort()),
			fields: Object.freeze([...fields].sort()),
			valueSources: Object.freeze([...sources].sort())
		});
	});
	return Object.freeze({
		kind: 'command' as const,
		name: artifact.name,
		operation: artifact.operationHash,
		consistency: artifact.consistency,
		effects: Object.freeze(effects),
		revalidation: Object.freeze({
			required: artifact.revalidation.required,
			dependencies: Object.freeze(
				[...artifact.revalidation.dependencies].sort()
			),
			models: Object.freeze([...artifact.revalidation.models].sort())
		})
	});
}

function inspectRoot(
	root: ReplicaRootSelection,
	path: string,
	injected: ReplicaOperationInjectedFieldInspection[],
	dependencies: Set<string>,
	indexes: ReplicaOperationIndexInspection[]
): void {
	for (const dependency of root.dependencies) dependencies.add(dependency);
	indexes.push(
		Object.freeze({
			path,
			field: root.field,
			cardinality: root.cardinality,
			dependencies: Object.freeze([...root.dependencies].sort()),
			...(root.coverage === undefined
				? {}
				: { coverage: root.coverage.kind }),
			filtered: root.filter !== undefined,
			ordered: root.order !== undefined,
			...(root.pagination === undefined
				? {}
				: { pagination: root.pagination.kind })
		})
	);
	inspectSelection(
		root.selection,
		path,
		injected,
		dependencies,
		indexes
	);
}

function safeArtifactSource(
	source: ReplicaOperationArtifact<unknown, GraphqlVariables>['source']
): ReplicaArtifactSourceLocation | undefined {
	if (source === undefined) return undefined;
	const path = source.path;
	const driveAbsolute = path.length >= 2 && path[1] === ':';
	if (
		path.length === 0 ||
		path.length > 4_096 ||
		/[\u0000-\u001f\u007f]/.test(path) ||
		path.startsWith('/') ||
		driveAbsolute ||
		path.includes('\\') ||
		path.split('/').includes('..') ||
		(!path.endsWith('.graphql') && !path.endsWith('.gql')) ||
		!Number.isSafeInteger(source.line) ||
		source.line < 1 ||
		!Number.isSafeInteger(source.column) ||
		source.column < 1
	) {
		return undefined;
	}
	return Object.freeze({
		path,
		line: source.line,
		column: source.column
	});
}

function inspectSelection(
	selection: ReplicaObjectSelection,
	path: string,
	injected: ReplicaOperationInjectedFieldInspection[],
	dependencies: Set<string>,
	indexes: ReplicaOperationIndexInspection[]
): void {
	for (const member of selection.members) {
		const memberPath = `${path}.${member.responseKey}`;
		if (member.kind === 'scalar') {
			if (member.expose === false) {
				injected.push(
					Object.freeze({
						path: memberPath,
						responseKey: member.responseKey,
						field: member.field
					})
				);
			}
			continue;
		}
		const indexDependencies = new Set([
			...member.dependencies,
			...(member.relationship?.dependencies ?? [])
		]);
		for (const dependency of indexDependencies) {
			dependencies.add(dependency);
		}
		indexes.push(
			Object.freeze({
				path: memberPath,
				field: member.field,
				cardinality: member.cardinality,
				dependencies: Object.freeze([...indexDependencies].sort()),
				...(member.coverage === undefined
					? {}
					: { coverage: member.coverage.kind }),
				filtered: member.filter !== undefined,
				ordered: member.order !== undefined,
				...(member.pagination === undefined
					? {}
					: { pagination: member.pagination.kind })
			})
		);
		inspectSelection(member.selection, memberPath, injected, dependencies, indexes);
	}
}

function expressionSource(
	expression: ReplicaCommandEffectExpression
): ReplicaCommandEffectInspection['valueSources'][number] {
	return expression.kind;
}

