import { lstat, readFile, readdir } from 'node:fs/promises';
import {
	dirname,
	extname,
	isAbsolute,
	join,
	posix,
	relative,
	resolve,
	sep
} from 'node:path';

import { parse } from 'svelte/compiler';

const BOUNDARY_PLAN_VERSION = 1;
const MAX_COMPONENTS = 4_096;
const MAX_COMPONENT_BYTES = 2 * 1024 * 1024;
const MAX_GRAPH_EDGES = 32_768;

type AstNode = Readonly<Record<string, unknown>> & {
	type?: string;
	start?: number;
	end?: number;
	loc?: Readonly<{
		start?: Readonly<{ line?: number; column?: number }>;
	}>;
};

export type DistributedIslandInventory = Readonly<{
	version: number;
	schemaFingerprint: string;
	protocolFingerprint: string;
	surface: unknown;
	islands: readonly DistributedIslandPlanInput[];
}>;

export type DistributedIslandPlanInput = Readonly<{
	version: number;
	id: string;
	operation: string;
	source: Readonly<{ path: string; line: number; column: number }>;
	directives: Readonly<{ load: boolean; live: boolean }>;
	variableSchema: Readonly<{
		reference: string;
		codecVersion: number;
		variables: readonly Readonly<{
			name: string;
			graphqlType: string;
		}>[];
	}>;
}>;

export type DistributedSvelteKitBoundaryOccurrence = Readonly<{
	islandId: string;
	operation: string;
	component: string;
	graphqlSource: string;
	reason: 'route_document' | 'static_component_import' | 'explicit';
	conservative: boolean;
}>;

export type DistributedSvelteKitBoundary = Readonly<{
	id: string;
	route: string;
	kind: 'layout' | 'page';
	source: string;
	islands: readonly DistributedSvelteKitBoundaryOccurrence[];
}>;

export type DistributedSvelteKitBoundaryPlan = Readonly<{
	version: number;
	module: string;
	schemaFingerprint: string;
	protocolFingerprint: string;
	boundaries: readonly DistributedSvelteKitBoundary[];
	unplaced: readonly Readonly<{
		islandId: string;
		operation: string;
		graphqlSource: string;
	}>[];
}>;

export type DistributedSvelteKitBoundaryAnalysisClient = Readonly<{
	module: string;
	inventory: DistributedIslandInventory;
	explicitBoundaries?: readonly DistributedSvelteKitBoundaryRegistration[];
}>;

export type DistributedSvelteKitBoundaryRegistration = Readonly<{
	operation: string;
	route: string;
	kind: 'layout' | 'page';
}>;

export type DistributedSvelteKitBoundaryAnalysisOptions = Readonly<{
	cwd: string;
	routesDir?: string;
	libDir?: string;
	aliases?: Readonly<Record<string, string>>;
	clients: readonly DistributedSvelteKitBoundaryAnalysisClient[];
}>;

type ComponentModule = Readonly<{
	path: string;
	imports: readonly StaticImport[];
	dynamicImports: readonly DynamicImport[];
}>;

type StaticImport = Readonly<{
	specifier: string;
	line: number;
	column: number;
}>;

type DynamicImport = StaticImport & Readonly<{ opaque: boolean }>;

type BoundaryRoot = Readonly<{
	id: string;
	route: string;
	kind: 'layout' | 'page';
	path: string;
}>;

/**
 * Analyze Svelte component reachability without evaluating application code.
 * The returned plan is deterministic and contains project-relative paths only.
 */
export async function analyzeDistributedSvelteKitBoundaries(
	options: DistributedSvelteKitBoundaryAnalysisOptions
): Promise<readonly DistributedSvelteKitBoundaryPlan[]> {
	const cwd = resolve(options.cwd);
	const routesDir = contained(cwd, options.routesDir ?? 'src/routes', 'routesDir');
	const libDir = contained(cwd, options.libDir ?? 'src/lib', 'libDir');
	const aliases = new Map<string, string>([
		['$lib', libDir],
		...Object.entries(options.aliases ?? {}).map(([key, value]) => [
			aliasKey(key),
			contained(cwd, value, `alias ${key}`)
		] as const)
	]);
	const components = await loadComponents(cwd, [
		...new Set([routesDir, libDir, ...aliases.values()])
	]);
	const roots = boundaryRoots(cwd, routesDir, components);
	const sourceOwners = new Map<string, string>();
	for (const client of options.clients) {
		validateInventory(client.module, client.inventory);
		for (const island of client.inventory.islands.filter(
			(candidate) => candidate.directives.load
		)) {
			const source = portableSource(island.source.path);
			const previous = sourceOwners.get(source);
			if (previous !== undefined && previous !== client.module) {
				throw diagnostic(
					'distributed.island.cross_surface',
					source,
					island.source.line,
					island.source.column,
					`operation ${island.operation} is owned by both ${previous} and ${client.module}; split the GraphQL source by authorization surface`
				);
			}
			sourceOwners.set(source, client.module);
		}
	}

	const plans: DistributedSvelteKitBoundaryPlan[] = [];
	for (const client of [...options.clients].sort((left, right) =>
		left.module.localeCompare(right.module)
	)) {
		const loadIslands = client.inventory.islands
			.filter((island) => island.directives.load)
			.map((island) => ({ island, source: portableSource(island.source.path) }))
			.sort((left, right) =>
				left.source.localeCompare(right.source) ||
				left.island.operation.localeCompare(right.island.operation)
			);
		const componentIslands = new Map<string, typeof loadIslands>();
		const routeIslands = new Map<string, typeof loadIslands>();
		const islandByOperation = new Map(
			loadIslands.map((entry) => [entry.island.operation, entry] as const)
		);
		const explicitByBoundary = new Map<string, typeof loadIslands>();
		const explicitlyPlaced = new Set<string>();
		for (const registration of client.explicitBoundaries ?? []) {
			const entry = islandByOperation.get(registration.operation);
			if (entry === undefined) {
				throw new Error(
					`[distributed.island.explicit_operation_missing] ${client.module} explicit boundary references unknown @load operation ${registration.operation}`
				);
			}
			const id = `${registration.kind}:${normalizeRoute(registration.route)}`;
			const bucket = explicitByBoundary.get(id) ?? [];
			bucket.push(entry);
			explicitByBoundary.set(id, bucket);
			explicitlyPlaced.add(entry.island.id);
		}
		for (const entry of loadIslands) {
			const target = islandOwnerComponent(cwd, entry.source);
			if (target.kind === 'component') {
				const bucket = componentIslands.get(target.path) ?? [];
				const conflicting = bucket.find((candidate) => candidate.source !== entry.source);
				if (conflicting !== undefined) {
					throw diagnostic(
						'distributed.island.sibling_conflict',
						entry.source,
						entry.island.source.line,
						entry.island.source.column,
						`operation ${entry.island.operation} conflicts with sibling source ${conflicting.source}; keep one GraphQL sibling or register an explicit boundary`
					);
				}
				bucket.push(entry);
				componentIslands.set(target.path, bucket);
			} else {
				const bucket = routeIslands.get(target.path) ?? [];
				bucket.push(entry);
				routeIslands.set(target.path, bucket);
			}
		}

		for (const component of componentIslands.keys()) {
			const needsComponent = componentIslands
				.get(component)!
				.some(({ island }) => !explicitlyPlaced.has(island.id));
			if (!needsComponent) continue;
			if (!components.has(component)) {
				const entry = componentIslands.get(component)![0]!;
				throw diagnostic(
					'distributed.island.component_missing',
					entry.source,
					entry.island.source.line,
					entry.island.source.column,
					`operation ${entry.island.operation} requires sibling ${projectPath(cwd, component)}; add the component, register an explicit boundary, or mark it client-only`
				);
			}
		}

		const boundaries: DistributedSvelteKitBoundary[] = [];
		const placed = new Set<string>();
		for (const root of roots) {
			const occurrences: DistributedSvelteKitBoundaryOccurrence[] = [];
			for (const entry of explicitByBoundary.get(root.id) ?? []) {
				occurrences.push(occurrence(cwd, entry, root.path, 'explicit', false));
				placed.add(entry.island.id);
			}
			for (const entry of routeIslands.get(root.path) ?? []) {
				occurrences.push(occurrence(cwd, entry, root.path, 'route_document', false));
				placed.add(entry.island.id);
			}
			const dynamicComponentIslands = new Map(
				[...componentIslands.entries()]
					.map(([path, entries]) => [
						path,
						entries.filter(({ island }) => !explicitlyPlaced.has(island.id))
					] as const)
					.filter(([, entries]) => entries.length > 0)
			);
			const reachable = traverse(root, components, aliases, dynamicComponentIslands, cwd);
			for (const component of reachable) {
				for (const entry of componentIslands.get(component) ?? []) {
					occurrences.push(
						occurrence(cwd, entry, component, 'static_component_import', true)
					);
					placed.add(entry.island.id);
				}
			}
			const deduplicated = [...new Map(
				occurrences.map((entry) => [entry.islandId, entry] as const)
			).values()].sort(compareOccurrences);
			if (deduplicated.length > 0) {
				boundaries.push(Object.freeze({
					id: root.id,
					route: root.route,
					kind: root.kind,
					source: projectPath(cwd, root.path),
					islands: Object.freeze(deduplicated)
				}));
			}
		}
		const unplaced = loadIslands
			.filter(({ island }) => !placed.has(island.id))
			.map(({ island, source }) => Object.freeze({
				islandId: island.id,
				operation: island.operation,
				graphqlSource: source
			}));
		plans.push(Object.freeze({
			version: BOUNDARY_PLAN_VERSION,
			module: client.module,
			schemaFingerprint: client.inventory.schemaFingerprint,
			protocolFingerprint: client.inventory.protocolFingerprint,
			boundaries: Object.freeze(boundaries),
			unplaced: Object.freeze(unplaced)
		}));
	}
	return Object.freeze(plans);
}

function occurrence(
	cwd: string,
	entry: { island: DistributedIslandPlanInput; source: string },
	component: string,
	reason: DistributedSvelteKitBoundaryOccurrence['reason'],
	conservative: boolean
): DistributedSvelteKitBoundaryOccurrence {
	return Object.freeze({
		islandId: entry.island.id,
		operation: entry.island.operation,
		component: projectPath(cwd, component),
		graphqlSource: entry.source,
		reason,
		conservative
	});
}

function traverse(
	root: BoundaryRoot,
	components: ReadonlyMap<string, ComponentModule>,
	aliases: ReadonlyMap<string, string>,
	componentIslands: ReadonlyMap<string, readonly unknown[]>,
	cwd: string
): readonly string[] {
	const reached = new Set<string>();
	const visiting = [root.path];
	let edges = 0;
	while (visiting.length > 0) {
		const path = visiting.pop()!;
		if (reached.has(path)) continue;
		reached.add(path);
		const component = components.get(path);
		if (component === undefined) continue;
		for (const imported of component.imports) {
			edges += 1;
			if (edges > MAX_GRAPH_EDGES) {
				throw diagnostic(
					'distributed.island.graph_unbounded',
					projectPath(cwd, path),
					imported.line,
					imported.column,
					`boundary ${root.route} exceeds ${MAX_GRAPH_EDGES} static component edges; split the boundary or register a bounded explicit boundary`
				);
			}
			const target = resolveComponentImport(path, imported, aliases, components, cwd);
			if (target !== undefined && !reached.has(target)) visiting.push(target);
		}
		for (const imported of component.dynamicImports) {
			if (imported.opaque) {
				if (componentIslands.size > 0) {
					throw diagnostic(
						'distributed.island.dynamic_opaque',
						projectPath(cwd, path),
						imported.line,
						imported.column,
						`boundary ${root.route} has opaque dynamic component reachability while @load islands remain unplaced; use a static import, explicit boundary registration, or client-only execution`
					);
				}
				continue;
			}
			const target = resolveComponentImport(path, imported, aliases, components, cwd);
			if (target !== undefined && componentIslands.has(target)) {
				throw diagnostic(
					'distributed.island.dynamic_load',
					projectPath(cwd, path),
					imported.line,
					imported.column,
					`boundary ${root.route} dynamically reaches an @load island; use a static import, explicit boundary registration, or client-only execution`
				);
			}
		}
	}
	return [...reached].sort();
}

function resolveComponentImport(
	from: string,
	imported: StaticImport,
	aliases: ReadonlyMap<string, string>,
	components: ReadonlyMap<string, ComponentModule>,
	cwd: string
): string | undefined {
	const specifier = imported.specifier;
	let candidate: string | undefined;
	if (specifier.startsWith('.')) {
		candidate = resolve(dirname(from), specifier);
	} else {
		const alias = [...aliases.entries()]
			.sort((left, right) => right[0].length - left[0].length)
			.find(([key]) => specifier === key || specifier.startsWith(`${key}/`));
		if (alias !== undefined) {
			candidate = resolve(alias[1], specifier.slice(alias[0].length).replace(/^\//, ''));
		} else if (specifier.startsWith('$') && looksLikeComponent(specifier)) {
			throw diagnostic(
				'distributed.island.alias_unresolved',
				projectPath(cwd, from),
				imported.line,
				imported.column,
				`component alias ${specifier.split('/')[0]} is unresolved; add it to distributedSvelteKit aliases or use a resolvable static import`
			);
		}
	}
	if (candidate === undefined) return undefined;
	if (!isWithin(cwd, candidate)) {
		throw diagnostic(
			'distributed.island.import_escape',
			projectPath(cwd, from),
			imported.line,
			imported.column,
			'component import escapes the project root; use a project-local component or client-only execution'
		);
	}
	const attempts = extname(candidate) === '.svelte'
		? [candidate]
		: [`${candidate}.svelte`, join(candidate, 'index.svelte')];
	for (const attempt of attempts) {
		if (components.has(attempt)) return attempt;
	}
	if (looksLikeComponent(specifier)) {
		throw diagnostic(
			'distributed.island.import_unresolved',
			projectPath(cwd, from),
			imported.line,
			imported.column,
			`component import ${specifier} is unresolved; fix the static import, register an explicit boundary, or mark it client-only`
		);
	}
	return undefined;
}

function looksLikeComponent(specifier: string): boolean {
	return specifier.endsWith('.svelte') || /\/[A-Z][^/]*$/.test(specifier);
}

async function loadComponents(
	cwd: string,
	roots: readonly string[]
): Promise<ReadonlyMap<string, ComponentModule>> {
	const paths = new Set<string>();
	for (const root of roots) await collectSvelteFiles(root, cwd, paths);
	if (paths.size > MAX_COMPONENTS) {
		throw new Error(
			`[distributed.island.graph_unbounded] Svelte project exceeds ${MAX_COMPONENTS} components; narrow routesDir/libDir`
		);
	}
	const components = new Map<string, ComponentModule>();
	for (const path of [...paths].sort()) {
		const metadata = await lstat(path);
		if (!metadata.isFile() || metadata.isSymbolicLink()) continue;
		if (metadata.size > MAX_COMPONENT_BYTES) {
			throw diagnostic(
				'distributed.island.component_too_large',
				projectPath(cwd, path),
				1,
				1,
				`component exceeds ${MAX_COMPONENT_BYTES} bytes; split it or use client-only execution`
			);
		}
		const source = await readFile(path, 'utf8');
		components.set(path, parseComponent(path, source, cwd));
	}
	return components;
}

async function collectSvelteFiles(
	root: string,
	cwd: string,
	paths: Set<string>
): Promise<void> {
	let entries;
	try {
		entries = await readdir(root, { withFileTypes: true });
	} catch (error) {
		if (isMissing(error)) return;
		throw error;
	}
	for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
		const path = join(root, entry.name);
		if (!isWithin(cwd, path) || entry.isSymbolicLink()) continue;
		if (entry.isDirectory()) await collectSvelteFiles(path, cwd, paths);
		else if (entry.isFile() && entry.name.endsWith('.svelte')) paths.add(path);
	}
}

function parseComponent(path: string, source: string, cwd: string): ComponentModule {
	let ast: AstNode;
	try {
		ast = parse(source, { filename: projectPath(cwd, path), modern: true }) as unknown as AstNode;
	} catch (error) {
		throw new Error(
			`[distributed.island.svelte_parse] ${projectPath(cwd, path)}: ${error instanceof Error ? error.message : String(error)}`
		);
	}
	const imports: StaticImport[] = [];
	const dynamicImports: DynamicImport[] = [];
	walkAst(ast, (node) => {
		if (node.type === 'ImportDeclaration') {
			const value = literalValue(node.source);
			if (value !== undefined) imports.push(located(value, node));
		} else if (node.type === 'ImportExpression') {
			const value = literalValue(node.source);
			dynamicImports.push({
				...located(value ?? '<dynamic>', node),
				opaque: value === undefined
			});
		}
	});
	return Object.freeze({
		path,
		imports: Object.freeze(imports),
		dynamicImports: Object.freeze(dynamicImports)
	});
}

function walkAst(value: unknown, visit: (node: AstNode) => void): void {
	const stack: unknown[] = [value];
	const seen = new WeakSet<object>();
	while (stack.length > 0) {
		const current = stack.pop();
		if (current === null || typeof current !== 'object') continue;
		if (seen.has(current)) continue;
		seen.add(current);
		if (!Array.isArray(current)) visit(current as AstNode);
		for (const child of Array.isArray(current)
			? current
			: Object.values(current as Record<string, unknown>)) {
			if (child !== null && typeof child === 'object') stack.push(child);
		}
	}
}

function literalValue(value: unknown): string | undefined {
	if (value === null || typeof value !== 'object') return undefined;
	const candidate = value as { type?: string; value?: unknown };
	return candidate.type === 'Literal' && typeof candidate.value === 'string'
		? candidate.value
		: undefined;
}

function located(specifier: string, node: AstNode): StaticImport {
	return Object.freeze({
		specifier,
		line: node.loc?.start?.line ?? 1,
		column: (node.loc?.start?.column ?? 0) + 1
	});
}

function boundaryRoots(
	cwd: string,
	routesDir: string,
	components: ReadonlyMap<string, ComponentModule>
): readonly BoundaryRoot[] {
	return [...components.keys()]
		.filter((path) => path.startsWith(`${routesDir}${sep}`) || path === routesDir)
		.flatMap((path): BoundaryRoot[] => {
			const name = posix.basename(portable(path));
			const kind = name === '+page.svelte' ? 'page' : name === '+layout.svelte' ? 'layout' : undefined;
			if (kind === undefined) return [];
			const routeDirectory = portable(relative(routesDir, dirname(path)));
			const route = routeDirectory === '' ? '/' : `/${routeDirectory}`;
			return [{
				id: `${kind}:${route}`,
				route,
				kind,
				path
			}];
		})
		.sort((left, right) =>
			left.route.localeCompare(right.route) ||
			left.kind.localeCompare(right.kind) ||
			projectPath(cwd, left.path).localeCompare(projectPath(cwd, right.path))
		);
}

function islandOwnerComponent(
	cwd: string,
	source: string
): Readonly<{ kind: 'component' | 'route'; path: string }> {
	const absolute = contained(cwd, source, 'island source');
	const suffix = extname(absolute);
	const base = absolute.slice(0, -suffix.length);
	const name = posix.basename(portable(base));
	if (name === '+page' || name === '+layout') {
		return { kind: 'route', path: `${base}.svelte` };
	}
	return { kind: 'component', path: `${base}.svelte` };
}

function validateInventory(module: string, inventory: DistributedIslandInventory): void {
	if (
		inventory === null ||
		typeof inventory !== 'object' ||
		inventory.version !== 1 ||
		!Array.isArray(inventory.islands) ||
		typeof inventory.schemaFingerprint !== 'string' ||
		typeof inventory.protocolFingerprint !== 'string'
	) {
		throw new Error(
			`[distributed.island.inventory_invalid] ${module} islands.json is not version 1`
		);
	}
}

function portableSource(path: string): string {
	if (
		typeof path !== 'string' ||
		path.length === 0 ||
		isAbsolute(path) ||
		path.includes('\\') ||
		path.split('/').some((part) => part === '' || part === '.' || part === '..')
	) {
		throw new Error('[distributed.island.source_invalid] island source path is not portable');
	}
	return path;
}

function compareOccurrences(
	left: DistributedSvelteKitBoundaryOccurrence,
	right: DistributedSvelteKitBoundaryOccurrence
): number {
	return (
		left.operation.localeCompare(right.operation) ||
		left.component.localeCompare(right.component) ||
		left.graphqlSource.localeCompare(right.graphqlSource)
	);
}

function aliasKey(value: string): string {
	if (!/^\$[A-Za-z0-9_-]+$/.test(value)) {
		throw new TypeError(`Distributed SvelteKit alias ${value} must be a single $name segment`);
	}
	return value;
}

function normalizeRoute(value: string): string {
	if (
		typeof value !== 'string' ||
		!value.startsWith('/') ||
		value.includes('\\') ||
		value.includes('?') ||
		value.includes('#') ||
		value.split('/').some((part) => part === '.' || part === '..')
	) {
		throw new TypeError('Distributed explicit boundary route must be a normalized SvelteKit route id');
	}
	return value.length > 1 && value.endsWith('/') ? value.slice(0, -1) : value;
}

function contained(root: string, value: string, label: string): string {
	const path = resolve(root, value);
	if (!isWithin(root, path)) throw new TypeError(`${label} must stay within project root`);
	return path;
}

function isWithin(root: string, target: string): boolean {
	const path = relative(root, target);
	return path === '' || (!path.startsWith(`..${sep}`) && path !== '..' && !isAbsolute(path));
}

function projectPath(cwd: string, path: string): string {
	return portable(relative(cwd, path));
}

function portable(path: string): string {
	return path.split(sep).join('/');
}

function diagnostic(
	code: string,
	path: string,
	line: number,
	column: number,
	message: string
): Error {
	return new Error(`[${code}] ${path}:${line}:${column}: ${message}`);
}

function isMissing(error: unknown): boolean {
	return error !== null && typeof error === 'object' && 'code' in error && error.code === 'ENOENT';
}
