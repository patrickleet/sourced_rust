/**
 * Materialize TypeScript `defineQuery` modules into GraphQL document text for
 * `dctl client`. GraphQL remains the only query language the compiler accepts.
 */
import * as esbuild from 'esbuild';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import {
	dirname,
	isAbsolute,
	join,
	relative,
	resolve,
	sep
} from 'node:path';
import { pathToFileURL } from 'node:url';
import { mkdtemp, rm } from 'node:fs/promises';

const QUERY_TS = /\.query\.tsx?$/;
const GRAPHQL = /\.(?:graphql|gql)$/;

export type MaterializeClientDocumentsOptions = Readonly<{
	/** Project root used to resolve document globs. */
	cwd: string;
	/** Same globs passed to `dctl client --documents`. */
	patterns: readonly string[];
	/**
	 * Directory that receives materialized GraphQL files. Relative paths under
	 * this directory preserve the authoring path so route convention still sees
	 * SvelteKit page documents such as +page.query.ts or +page.graphql.
	 */
	outDir: string;
}>;

export type MaterializedClientDocuments = Readonly<{
	/** Absolute paths to feed as repeated `dctl client --documents` values. */
	documents: readonly string[];
}>;

/**
 * Expand document globs, evaluate `*.query.ts` builders to GraphQL, copy plain
 * GraphQL sources, and return absolute paths for `dctl client`.
 */
export async function materializeClientDocuments(
	options: MaterializeClientDocumentsOptions
): Promise<MaterializedClientDocuments> {
	const cwd = resolve(options.cwd);
	const outDir = resolve(options.outDir);
	if (options.patterns.length === 0) {
		throw new TypeError('materializeClientDocuments requires at least one document pattern');
	}

	const matched = new Map<string, string>();
	for (const pattern of options.patterns) {
		if (typeof pattern !== 'string' || pattern.trim().length === 0) {
			throw new TypeError('document pattern must be a non-empty string');
		}
		const hits = await expandDocumentPattern(cwd, pattern);
		if (hits.length === 0) {
			throw new Error(`document glob \`${pattern}\` matched no files under ${cwd}`);
		}
		for (const absolute of hits) {
			const rel = portableRelative(cwd, absolute);
			matched.set(absolute, rel);
		}
	}

	const documents: string[] = [];
	for (const [absolute, rel] of [...matched.entries()].sort((a, b) =>
		a[1].localeCompare(b[1])
	)) {
		const dest = join(outDir, rel);
		await mkdir(dirname(dest), { recursive: true });
		if (QUERY_TS.test(rel)) {
			const graphql = await evaluateQueryModule(absolute);
			await writeFile(dest, graphql, 'utf8');
		} else if (GRAPHQL.test(rel)) {
			const source = await readFile(absolute, 'utf8');
			await writeFile(dest, source, 'utf8');
		} else {
			throw new Error(
				`unsupported client document \`${rel}\`; use .graphql/.gql or .query.ts`
			);
		}
		documents.push(dest);
	}

	return Object.freeze({ documents: Object.freeze(documents) });
}

/** Evaluate one `defineQuery` module and return GraphQL document text. */
export async function evaluateQueryModule(absolutePath: string): Promise<string> {
	const entry = resolve(absolutePath);
	const buildDir = await mkdtemp(join(tmpdir(), 'distributed-query-'));
	const outfile = join(buildDir, 'query.mjs');
	try {
		await esbuild.build({
			entryPoints: [entry],
			bundle: true,
			platform: 'node',
			format: 'esm',
			target: 'node20',
			outfile,
			logLevel: 'silent',
			packages: 'bundle'
		});
		const href = `${pathToFileURL(outfile).href}?t=${Date.now()}`;
		const mod = (await import(href)) as Record<string, unknown>;
		return extractGraphqlDocument(mod, entry);
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		throw new Error(`failed to materialize query module ${entry}: ${message}`, {
			cause: error
		});
	} finally {
		await rm(buildDir, { recursive: true, force: true });
	}
}

function extractGraphqlDocument(
	mod: Record<string, unknown>,
	path: string
): string {
	const candidates = [mod.default, mod.query, mod.document];
	for (const candidate of candidates) {
		const graphql = coerceGraphql(candidate);
		if (graphql !== undefined) return ensureTrailingNewline(graphql);
	}
	throw new Error(
		`${path} must default-export a defineQuery() builder or GraphQL string (or export query/document)`
	);
}

function coerceGraphql(value: unknown): string | undefined {
	if (typeof value === 'string') {
		const trimmed = value.trim();
		if (trimmed.length === 0) return undefined;
		return value;
	}
	if (
		value !== null &&
		typeof value === 'object' &&
		'toGraphql' in value &&
		typeof (value as { toGraphql?: unknown }).toGraphql === 'function'
	) {
		const graphql = (value as { toGraphql: () => unknown }).toGraphql();
		if (typeof graphql === 'string' && graphql.trim().length > 0) {
			return graphql;
		}
	}
	return undefined;
}

function ensureTrailingNewline(value: string): string {
	return value.endsWith('\n') ? value : `${value}\n`;
}

async function expandDocumentPattern(
	cwd: string,
	pattern: string
): Promise<string[]> {
	const normalized = pattern.replaceAll('\\', '/');
	if (!normalized.includes('*') && !normalized.includes('?') && !normalized.includes('[')) {
		const absolute = resolve(cwd, normalized);
		try {
			const source = await readFile(absolute);
			void source;
			return [absolute];
		} catch {
			return [];
		}
	}

	const expression = globToRegExp(normalized);
	const files = await listFiles(cwd);
	return files
		.filter((absolute) => {
			const rel = portableRelative(cwd, absolute);
			return expression.test(rel);
		})
		.sort();
}

async function listFiles(root: string): Promise<string[]> {
	const out: string[] = [];
	async function walk(dir: string): Promise<void> {
		let entries;
		try {
			entries = await readdir(dir, { withFileTypes: true });
		} catch {
			return;
		}
		for (const entry of entries) {
			if (entry.name === 'node_modules' || entry.name === '.git' || entry.name === 'dist') {
				continue;
			}
			const full = join(dir, entry.name);
			if (entry.isDirectory()) {
				await walk(full);
			} else if (entry.isFile()) {
				out.push(full);
			}
		}
	}
	await walk(root);
	return out;
}

function globToRegExp(pattern: string): RegExp {
	let i = 0;
	let body = '';
	while (i < pattern.length) {
		const ch = pattern[i]!;
		if (ch === '*' && pattern[i + 1] === '*') {
			body += '.*';
			i += 2;
			if (pattern[i] === '/') i += 1;
			continue;
		}
		if (ch === '*') {
			body += '[^/]*';
			i += 1;
			continue;
		}
		if (ch === '?') {
			body += '[^/]';
			i += 1;
			continue;
		}
		if ('\\.^$+{}()|[]'.includes(ch)) {
			body += `\\${ch}`;
			i += 1;
			continue;
		}
		body += ch;
		i += 1;
	}
	return new RegExp(`^${body}$`);
}

function portableRelative(cwd: string, absolute: string): string {
	const rel = relative(cwd, absolute);
	if (isAbsolute(rel) || rel.startsWith(`..${sep}`) || rel === '..') {
		throw new Error(`document ${absolute} resolves outside project root ${cwd}`);
	}
	return rel.split(sep).join('/');
}
