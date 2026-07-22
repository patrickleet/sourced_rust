import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);
const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const nodeCommand = process.execPath;

async function run(command, args, cwd) {
	try {
		return await execFileAsync(command, args, {
			cwd,
			env: process.env,
			maxBuffer: 10 * 1024 * 1024
		});
	} catch (error) {
		const stdout = typeof error.stdout === 'string' ? error.stdout.trim() : '';
		const stderr = typeof error.stderr === 'string' ? error.stderr.trim() : '';
		const output = [stdout, stderr].filter(Boolean).join('\n');
		throw new Error(
			`Command failed: ${command} ${args.join(' ')}${output ? `\n${output}` : ''}`,
			{ cause: error }
		);
	}
}

function collectPackageTargets(value, targets) {
	if (typeof value === 'string') {
		if (value.startsWith('./')) targets.add(value.slice(2));
		else if (!value.startsWith('.') && !isAbsolute(value)) targets.add(value);
		return;
	}
	if (Array.isArray(value)) {
		for (const entry of value) collectPackageTargets(entry, targets);
		return;
	}
	if (value && typeof value === 'object') {
		for (const entry of Object.values(value)) collectPackageTargets(entry, targets);
	}
}

function parsePackResult(stdout) {
	const trimmed = stdout.trim();
	try {
		return JSON.parse(trimmed);
	} catch (error) {
		throw new Error(`npm pack did not return valid JSON:\n${trimmed}`, { cause: error });
	}
}

function inspectPackResult(packResult, packageJson) {
	assert.equal(packResult.length, 1, 'npm pack must produce exactly one tarball');
	const [packed] = packResult;
	assert.equal(packed.name, packageJson.name, 'packed package name must match package.json');
	assert.equal(
		packed.version,
		packageJson.version,
		'packed package version must match package.json'
	);
	assert.ok(Array.isArray(packed.files), 'npm pack result must include its file manifest');

	const packedPaths = new Set(packed.files.map((file) => file.path));
	const requiredPaths = new Set(['README.md', 'package.json']);
	collectPackageTargets(packageJson.exports, requiredPaths);
	collectPackageTargets(packageJson.bin, requiredPaths);

	for (const requiredPath of requiredPaths) {
		assert.ok(packedPaths.has(requiredPath), `packed tarball is missing ${requiredPath}`);
	}
	assert.ok(
		[...packedPaths].some((path) => path.startsWith('dist/')),
		'packed tarball must contain built dist files'
	);

	const forbiddenSegments = new Set(['src', 'tests', 'node_modules']);
	const forbiddenPaths = [...packedPaths].filter((path) =>
		path.split('/').some((segment) => forbiddenSegments.has(segment))
	);
	assert.deepEqual(
		forbiddenPaths,
		[],
		`packed tarball contains forbidden paths: ${forbiddenPaths.join(', ')}`
	);

	return { packed, packedPaths };
}

const typecheckSource = `
import {
  QueryCache,
  bindCommands,
  createGraphqlClient,
  defineCommand,
  defineCommands,
  defineResource,
  type GqlDocument,
  type GqlResult,
  type GraphqlClient
} from '@hops-ops/distributed';
import { cacheKey } from '@hops-ops/distributed/cache';
import type { CommandClient } from '@hops-ops/distributed/commands';
import {
  createDistributedReplica,
  type ReplicaSnapshot,
  type ReplicaSparse
} from '@hops-ops/distributed/replica';
import { fieldToFunctionName } from '@hops-ops/distributed/codegen';
import {
  createLoadQuery,
  createUseGraphql,
  distributedGraphqlProxy,
  type PageGraphqlData,
  type ServerLoadEventLike
} from '@hops-ops/distributed/sveltekit';

type HealthData = { health: string };
type Session = { accessToken?: string | null; user?: { id?: string | null } | null };
type LoadEvent = ServerLoadEventLike<{ session: Session | null }>;
type TodosResult = { todos: Array<{ id: string; title: string }> };
declare const replicaSnapshot: ReplicaSnapshot<TodosResult>;
const sparseTodos: ReplicaSparse<TodosResult> = {};
if (replicaSnapshot.complete) {
  replicaSnapshot.data.todos.map((todo) => todo.title);
} else {
  // @ts-expect-error Sparse/loading data requires a presence guard.
  replicaSnapshot.data.todos.map((todo) => todo.title);
}

const query: GqlDocument<HealthData> = 'query Smoke { health }';
const resource = defineResource<HealthData>({
  query,
  select: (data) => data.health
});
const cache = new QueryCache();
const client: GraphqlClient = createGraphqlClient({
  getUrl: () => '/graphql',
  getAuth: async () => ({ userId: 'pack-smoke', role: 'user' }),
  cache
});

const pageData: PageGraphqlData = {
  session: { user: { id: 'pack-smoke' } },
  engineRole: 'user'
};
const useGraphql = createUseGraphql({ url: '/graphql' });
const pageClient = useGraphql(() => pageData, { cache });
// @ts-expect-error A read-only adapter must not invent generated command names.
pageClient.commands.missing();
const proxy = distributedGraphqlProxy('http://127.0.0.1:8791');
const commandClient: CommandClient = client;
const commandDefinitions = defineCommands({
  create: defineCommand<{ id: string }, { id: string }>({
    field: 'create',
    document: 'mutation Create($input: CreateInput!) { create(input: $input) { id } }',
    hasInput: true
  }),
  ping: defineCommand<void, { ok: boolean }>({
    field: 'ping',
    document: 'mutation Ping { ping { ok } }',
    hasInput: false
  })
});
const commands = bindCommands(commandClient, commandDefinitions);
void commands.create({ id: 'one' });
void commands.ping();
void cacheKey('query Smoke { health }');
void fieldToFunctionName('todos_create');

const loadQuery = createLoadQuery<Session, LoadEvent>({
  getSession: async (event) => event.locals.session,
  getRole: () => 'user',
  request: async <TData>(): Promise<GqlResult<TData>> => ({ status: 200 })
});
const loadHealth = loadQuery<HealthData, { health: string }>(
  query,
  (data) => ({ health: data?.health ?? 'unknown' })
);

void [resource, client, pageClient, proxy, loadHealth, createDistributedReplica, sparseTodos];
`;

const runtimeSource = `
import assert from 'node:assert/strict';
import {
  QueryCache,
  createGraphqlClient,
  defineResource,
  looksLikeMutation
} from '@hops-ops/distributed';
import {
  createUseGraphql,
  distributedGraphqlProxy
} from '@hops-ops/distributed/sveltekit';
import { createDistributedReplica } from '@hops-ops/distributed/replica';

const document = 'query Smoke { health }';
const resource = defineResource({
  query: document,
  select: (data) => data.health
});
const cache = new QueryCache();
const client = createGraphqlClient({
  getUrl: () => 'http://127.0.0.1:8791/graphql',
  getAuth: () => ({ userId: 'pack-smoke', role: 'user' }),
  cache,
  fetch: async (_input, init) => {
    const body = JSON.parse(String(init?.body));
    assert.equal(body.query, document);
    return new Response(JSON.stringify({ data: { health: 'ok' } }), {
      status: 200,
      headers: { 'content-type': 'application/json' }
    });
  }
});

const result = await client.request(document);
assert.equal(result.status, 200);
assert.equal(resource.select(result.data), 'ok');
assert.equal(looksLikeMutation('# comment\\nmutation Smoke { run }'), true);

const proxy = distributedGraphqlProxy('http://127.0.0.1:8791');
assert.deepEqual(proxy, {
  '/graphql': {
    target: 'http://127.0.0.1:8791',
    changeOrigin: true,
    ws: true
  }
});

const useGraphql = createUseGraphql({ url: '/graphql' });
const pageClient = useGraphql(() => ({ session: null, engineRole: null }));
assert.equal(typeof pageClient.request, 'function');
assert.equal(typeof pageClient.store, 'function');
assert.equal(typeof createDistributedReplica().read, 'function');
`;

async function smokePack() {
	const packageJson = JSON.parse(
		await readFile(join(packageRoot, 'package.json'), 'utf8')
	);
	const typescriptVersion = packageJson.devDependencies?.typescript;
	assert.ok(typescriptVersion, 'package devDependencies must declare TypeScript');

	const temporaryRoot = await mkdtemp(join(tmpdir(), 'distributed-pack-smoke-'));
	try {
		const packDirectory = join(temporaryRoot, 'pack');
		const consumerDirectory = join(temporaryRoot, 'consumer');
		await mkdir(packDirectory);
		await mkdir(consumerDirectory);

		const { stdout } = await run(
			npmCommand,
			['pack', '--json', '--silent', '--pack-destination', packDirectory],
			packageRoot
		);
		const { packed, packedPaths } = inspectPackResult(
			parsePackResult(stdout),
			packageJson
		);
		const tarballPath = isAbsolute(packed.filename)
			? packed.filename
			: resolve(packDirectory, packed.filename);
		assert.ok(
			tarballPath.startsWith(`${resolve(packDirectory)}${sep}`),
			'npm pack returned a tarball outside the temporary pack directory'
		);
		assert.ok((await stat(tarballPath)).size > 0, 'packed tarball must not be empty');

		await writeFile(
			join(consumerDirectory, 'package.json'),
			`${JSON.stringify(
				{
					name: 'distributed-pack-smoke-consumer',
					private: true,
					type: 'module'
				},
				null,
				2
			)}\n`
		);
		await run(
			npmCommand,
			[
				'install',
				'--ignore-scripts',
				'--no-audit',
				'--no-fund',
				'--save-exact',
				tarballPath,
				`typescript@${typescriptVersion}`
			],
			consumerDirectory
		);

		const installedPackageJson = JSON.parse(
			await readFile(
				join(
					consumerDirectory,
					'node_modules',
					...packageJson.name.split('/'),
					'package.json'
				),
				'utf8'
			)
		);
		assert.equal(installedPackageJson.name, packageJson.name);
		assert.equal(installedPackageJson.version, packageJson.version);
		assert.deepEqual(
			installedPackageJson.bin,
			packageJson.bin,
			'npm pack must not normalize or remove the declared CLI bin mapping'
		);

		await writeFile(
			join(consumerDirectory, 'tsconfig.json'),
			`${JSON.stringify(
				{
					compilerOptions: {
						target: 'ES2022',
						module: 'NodeNext',
						moduleResolution: 'NodeNext',
						lib: ['ES2022', 'DOM'],
						types: [],
						strict: true,
						noEmit: true,
						verbatimModuleSyntax: true
					},
					include: ['consumer.ts']
				},
				null,
				2
			)}\n`
		);
		await writeFile(join(consumerDirectory, 'consumer.ts'), typecheckSource);
		await writeFile(join(consumerDirectory, 'runtime.mjs'), runtimeSource);

		const tscPath = join(
			consumerDirectory,
			'node_modules',
			'typescript',
			'bin',
			'tsc'
		);
		await run(nodeCommand, [tscPath, '--project', 'tsconfig.json'], consumerDirectory);
		await run(nodeCommand, ['runtime.mjs'], consumerDirectory);
		const binPath = join(
			consumerDirectory,
			'node_modules',
			'.bin',
			process.platform === 'win32' ? 'distributed-gen-commands.cmd' : 'distributed-gen-commands'
		);
		const { stdout: cliHelp } = await run(binPath, ['--help'], consumerDirectory);
		assert.match(cliHelp, /Usage: distributed-gen-commands/);

		console.log(
			`Pack smoke passed for ${packageJson.name}@${packageJson.version} (${packedPaths.size} files).`
		);
	} finally {
		await rm(temporaryRoot, { recursive: true, force: true });
	}
}

await smokePack();
