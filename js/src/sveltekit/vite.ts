import { spawn, type ChildProcess } from 'node:child_process';
import {
	existsSync,
	lstatSync,
	realpathSync
} from 'node:fs';
import {
	cp,
	lstat,
	mkdir,
	mkdtemp,
	open,
	readFile,
	realpath,
	rename,
	rm,
	unlink,
	writeFile
} from 'node:fs/promises';
import {
	basename,
	dirname,
	isAbsolute,
	join,
	relative,
	resolve,
	sep
} from 'node:path';
import { isMainThread } from 'node:worker_threads';

const GENERATED_SVELTEKIT_MODULE = 'sveltekit.ts';
const MAX_COMMAND_OUTPUT_BYTES = 16 * 1024 * 1024;
const MODULE_NAME = /^\$distributed(?:\/[A-Za-z0-9][A-Za-z0-9._-]*)*$/;
const COMPILER_LOCK = join('.svelte-kit', 'distributed', 'compiler.lock');
const COMPILER_COORDINATORS = Symbol.for(
	'@hops-ops/distributed/sveltekit/compiler-coordinators/v1'
);

export type DistributedGraphqlProxyOptions = {
	/** Absolute Distributed API origin, for example `http://127.0.0.1:8791`. */
	target: string;
	/** Proxy key. Defaults to `/graphql` and includes its `/ws` child. */
	path?: string;
};

/** Vite proxy config for GraphQL HTTP and WebSocket traffic. */
export function distributedGraphqlProxy(
	options: DistributedGraphqlProxyOptions | string
): Record<string, { target: string; changeOrigin: true; ws: true }> {
	const target = (
		typeof options === 'string' ? options : options.target
	)
		.trim()
		.replace(/\/$/, '');
	const path =
		typeof options === 'string' ? '/graphql' : (options.path ?? '/graphql');
	if (!/^https?:\/\//.test(target)) {
		throw new Error(
			'distributedGraphqlProxy target must be an absolute http(s) URL'
		);
	}
	if (!path.startsWith('/')) {
		throw new Error('distributedGraphqlProxy path must start with /');
	}
	return { [path]: { target, changeOrigin: true, ws: true } };
}

export type DistributedSvelteKitManifestSource =
	| string
	| Readonly<{
			/**
			 * Arguments passed to the configured dctl command. The first value
			 * must be `client-manifest`; stdout becomes ephemeral compiler input.
			 */
			args: readonly string[];
	  }>;

export type DistributedSvelteKitClientCompiler = Readonly<{
	/** `$distributed` or an explicit elevated entrypoint such as `$distributed/admin`. */
	module: string;
	/** Existing manifest path, or canonical `dctl client-manifest` argv. */
	manifest: DistributedSvelteKitManifestSource;
	/** Verify exactly one concrete role. Mutually exclusive with `surface`. */
	role?: string;
	/** Verify exactly one Rust-declared application surface. */
	surface?: string;
	/**
	 * Client document globs passed as repeated `dctl client --documents`.
	 * Accepts GraphQL (`.graphql`/`.gql`) and QuerySpec (`.query.json`) sources.
	 */
	documents: readonly string[];
	/** Explicit `OPERATION=/route` fallbacks. */
	routes?: readonly string[];
	/** Compiler-owned artifact directory, relative to `cwd` by default. */
	out: string;
}>;

export type DistributedSvelteKitViteOptions = Readonly<{
	/** Project root used for dctl cwd, document globs, and output containment. */
	cwd?: string;
	/** Executable invoked without a shell. Defaults to `dctl`. */
	command?: string;
	/** Prefix argv, e.g. `cargo run ... --`; never interpreted by a shell. */
	commandArgs?: readonly string[];
	clients: readonly DistributedSvelteKitClientCompiler[];
}>;

type ResolvedClient = Readonly<{
	module: string;
	manifest: DistributedSvelteKitManifestSource;
	selector: readonly ['--role' | '--surface', string];
	documents: readonly string[];
	routes: readonly string[];
	out: string;
	entry: string;
	watchRoots: readonly string[];
}>;

type ResolvedIntegration = Readonly<{
	cwd: string;
	command: string;
	commandArgs: readonly string[];
	clients: readonly ResolvedClient[];
}>;

type CompilerCoordinator = {
	path: string;
	references: number;
	ready: Promise<void>;
	tail: Promise<void>;
	startupRuns: Map<string, Promise<void>>;
	closing?: Promise<void>;
};

type CompilerLockLease = {
	coordinator: CompilerCoordinator;
	released: boolean;
};

type ViteWebSocketLike = Readonly<{
	send(payload: unknown): void;
}>;

type ViteModuleGraphLike = Readonly<{
	getModuleById(id: string): unknown;
	invalidateModule(module: unknown): void;
}>;

type ViteServerLike = Readonly<{
	watcher: Readonly<{ add(paths: string | readonly string[]): void }>;
	ws: ViteWebSocketLike;
	moduleGraph: ViteModuleGraphLike;
	httpServer?:
		| Readonly<{
				once(event: 'close', listener: () => void): void;
		  }>
		| null;
}>;

type ViteHotContextLike = Readonly<{
	file: string;
	server: ViteServerLike;
}>;

type RollupWatchContextLike = Readonly<{
	addWatchFile(path: string): void;
}>;

/** Minimal structural Vite plugin type; avoids making Vite a runtime dependency. */
export type DistributedSvelteKitVitePlugin = Readonly<{
	name: string;
	enforce: 'pre';
	configResolved(config: Readonly<{ root: string }>): Promise<void>;
	configureServer(server: ViteServerLike): void;
	buildStart(this: RollupWatchContextLike): void;
	resolveId(source: string): string | undefined;
	load(id: string): string | undefined;
	handleHotUpdate(context: ViteHotContextLike): Promise<never[] | undefined>;
	watchChange(id: string): Promise<void>;
	closeBundle(): Promise<void>;
}>;

/** Generate every configured surface through the same transaction used by Vite. */
export async function generateDistributedSvelteKit(
	options: DistributedSvelteKitViteOptions
): Promise<void> {
	await runCompilerOnce(options, 'generate');
}

/** Check every configured surface through canonical `dctl client --check`; never write. */
export async function checkDistributedSvelteKit(
	options: DistributedSvelteKitViteOptions
): Promise<void> {
	await runCompilerOnce(options, 'check');
}

/**
 * Compiler-watching Vite integration for one or more authorization surfaces.
 *
 * Every run stages every surface first, then swaps the complete generated
 * trees as one rollback-capable transaction. Child processes are serialized,
 * coalesced, abortable, and always invoked without a shell.
 */
export function distributedSvelteKit(
	options: DistributedSvelteKitViteOptions
): DistributedSvelteKitVitePlugin {
	let resolved: ResolvedIntegration | undefined;
	let dirty = false;
	let running: Promise<void> | undefined;
	let completedGeneration = 0;
	let reloadedGeneration = 0;
	let stopped = false;
	const children = new Set<ChildProcess>();
	const cancellation = new AbortController();
	let lock: CompilerLockLease | undefined;

	const stop = async (): Promise<void> => {
		if (stopped) return;
		stopped = true;
		dirty = false;
		cancellation.abort();
		for (const child of children) killChild(child, 'SIGTERM');
		const force = setTimeout(() => {
			for (const child of children) killChild(child, 'SIGKILL');
		}, 1_500);
		force.unref();
		await Promise.race([
			running?.catch(() => undefined) ?? Promise.resolve(),
			new Promise<void>((resolvePromise) => {
				const bounded = setTimeout(resolvePromise, 3_000);
				bounded.unref();
			})
		]);
		clearTimeout(force);
		if (lock !== undefined) {
			await releaseCompilerLock(lock);
			lock = undefined;
		}
	};

	const compile = (reason: string, startup = false): Promise<void> => {
		if (stopped) {
			return Promise.reject(
				new Error(`Distributed SvelteKit compiler is closed (${reason})`)
			);
		}
		dirty = true;
		if (running !== undefined) return running;
		running = (async () => {
			let startupPending = startup;
			while (dirty && !stopped) {
				dirty = false;
				const lease = requireCompilerLock(lock);
				const integration = requireResolved(resolved);
				const operation = () =>
					compileTransaction(integration, children, cancellation.signal);
				await (startupPending
					? withCompilerStartup(lease, integration, operation)
					: withCompilerLock(lease, operation));
				startupPending = false;
			}
			completedGeneration += 1;
		})().finally(() => {
			running = undefined;
		});
		return running;
	};

	return {
		name: '@hops-ops/distributed:sveltekit',
		enforce: 'pre',
		async configResolved(config): Promise<void> {
			if (resolved !== undefined) {
				throw new Error(
					'Distributed SvelteKit Vite plugin was configured more than once'
				);
			}
			resolved = resolveIntegration(options, options.cwd ?? config.root);
			await validateResolvedPaths(resolved);
			/*
			 * SvelteKit post-build analysis loads Vite config in an isolated
			 * worker marked with SVELTEKIT_FORK. That pass reads framework
			 * configuration only; running the compiler there would contend with
			 * the owning build's physical project lock and duplicate generation.
			 */
			if (!isMainThread && process.env.SVELTEKIT_FORK === 'true') return;
			lock = await acquireCompilerLock(resolved.cwd);
			try {
				await compile('Vite startup', true);
			} catch (error) {
				await releaseCompilerLock(lock);
				lock = undefined;
				throw error;
			}
		},
		configureServer(server): void {
			const integration = requireResolved(resolved);
			const roots = integration.clients.flatMap((client) => client.watchRoots);
			if (roots.length > 0) server.watcher.add(roots);
			server.httpServer?.once('close', () => {
				void stop();
			});
		},
		buildStart(): void {
			const integration = requireResolved(resolved);
			for (const client of integration.clients) {
				for (const root of client.watchRoots) this.addWatchFile(root);
			}
		},
		resolveId(source): string | undefined {
			const client = resolved?.clients.find(
				(candidate) => candidate.module === source
			);
			return client === undefined ? undefined : virtualId(client.module);
		},
		load(id): string | undefined {
			const client = resolved?.clients.find(
				(candidate) => virtualId(candidate.module) === id
			);
			if (client === undefined) return undefined;
			return `export * from ${JSON.stringify(portablePath(client.entry))};\n`;
		},
		async handleHotUpdate(context): Promise<never[] | undefined> {
			const integration = requireResolved(resolved);
			if (!isClientDocumentInput(context.file, integration)) return undefined;
			try {
				await compile(`GraphQL change ${context.file}`);
			} catch (error) {
				context.server.ws.send({
					type: 'error',
					err: viteError(error)
				});
				throw error;
			}
			if (completedGeneration <= reloadedGeneration) return [];
			for (const client of integration.clients) {
				const module = context.server.moduleGraph.getModuleById(
					virtualId(client.module)
				);
				if (module !== undefined) {
					context.server.moduleGraph.invalidateModule(module);
				}
			}
			context.server.ws.send({ type: 'full-reload', path: '*' });
			reloadedGeneration = completedGeneration;
			return [];
		},
		async watchChange(id): Promise<void> {
			const integration = requireResolved(resolved);
			if (isClientDocumentInput(id, integration)) {
				await compile(`GraphQL watch change ${id}`);
			}
		},
		closeBundle: stop
	};
}

async function runCompilerOnce(
	options: DistributedSvelteKitViteOptions,
	mode: 'generate' | 'check'
): Promise<void> {
	const integration = resolveIntegration(
		options,
		options.cwd ?? process.cwd()
	);
	await validateResolvedPaths(integration);
	const lock = await acquireCompilerLock(integration.cwd);
	const children = new Set<ChildProcess>();
	const cancellation = new AbortController();
	const cancel = (): void => {
		cancellation.abort();
		for (const child of children) killChild(child, 'SIGTERM');
	};
	process.once('SIGINT', cancel);
	process.once('SIGTERM', cancel);
	try {
		await withCompilerLock(lock, () =>
			mode === 'generate'
				? compileTransaction(integration, children, cancellation.signal)
				: checkTransaction(integration, children, cancellation.signal)
		);
	} finally {
		process.removeListener('SIGINT', cancel);
		process.removeListener('SIGTERM', cancel);
		for (const child of children) killChild(child, 'SIGKILL');
		await releaseCompilerLock(lock);
	}
}

/**
 * SvelteKit language-tool aliases for the exact files used by Vite resolution.
 *
 * Keep this alongside the Vite plugin in the Node-only export. It performs no
 * generation and is safe to call from `svelte.config.js`.
 */
export function distributedSvelteKitAliases(
	options: Pick<DistributedSvelteKitViteOptions, 'cwd' | 'clients'>
): Readonly<Record<string, string>> {
	const integration = resolveIntegration(
		{ ...options, command: 'dctl', commandArgs: [] },
		options.cwd ?? process.cwd()
	);
	validateResolvedPathsSync(integration);
	return Object.freeze(
		Object.fromEntries(
			[...integration.clients]
				.sort(
					(left, right) =>
						right.module.length - left.module.length ||
						left.module.localeCompare(right.module)
				)
				.map((client) => [client.module, client.entry])
		)
	);
}

function resolveIntegration(
	options: DistributedSvelteKitViteOptions,
	fallbackCwd: string
): ResolvedIntegration {
	if (options === null || typeof options !== 'object') {
		throw new TypeError('distributedSvelteKit requires configuration');
	}
	const cwd = resolve(options.cwd ?? fallbackCwd);
	const command = (options.command ?? 'dctl').trim();
	if (command.length === 0) {
		throw new TypeError('Distributed SvelteKit command must not be empty');
	}
	if (!Array.isArray(options.clients) || options.clients.length === 0) {
		throw new TypeError(
			'Distributed SvelteKit requires at least one client surface'
		);
	}
	const modules = new Set<string>();
	const outputs: string[] = [];
	const clients = options.clients.map((client, index): ResolvedClient => {
		if (client === null || typeof client !== 'object') {
			throw new TypeError(`Distributed client[${index}] must be an object`);
		}
		if (!MODULE_NAME.test(client.module)) {
			throw new TypeError(
				`Distributed client module \`${client.module}\` must be $distributed or a safe $distributed/<entrypoint>`
			);
		}
		if (modules.has(client.module)) {
			throw new TypeError(
				`duplicate Distributed client module \`${client.module}\``
			);
		}
		modules.add(client.module);
		const selector =
			client.role !== undefined && client.surface === undefined
				? (['--role', nonempty(client.role, `${client.module} role`)] as const)
				: client.surface !== undefined && client.role === undefined
					? ([
							'--surface',
							nonempty(client.surface, `${client.module} surface`)
						] as const)
					: undefined;
		if (selector === undefined) {
			throw new TypeError(
				`Distributed client \`${client.module}\` requires exactly one of role or surface`
			);
		}
		if (
			!Array.isArray(client.documents) ||
			client.documents.length === 0
		) {
			throw new TypeError(
				`Distributed client \`${client.module}\` requires at least one client document glob (GraphQL or QuerySpec)`
			);
		}
		const documents = client.documents.map((document: string, documentIndex: number) =>
			nonempty(
				document,
				`${client.module} documents[${documentIndex}]`
			)
		);
		const routes = (client.routes ?? []).map((route: string, routeIndex: number) =>
			nonempty(route, `${client.module} routes[${routeIndex}]`)
		);
		validateManifestSource(client.module, client.manifest);
		const out = containedPath(
			cwd,
			client.out,
			`${client.module} generated output`
		);
		if (out === cwd) {
			throw new TypeError(
				`Distributed client \`${client.module}\` cannot use the project root as generated output`
			);
		}
		for (const existing of outputs) {
			if (isWithin(existing, out) || isWithin(out, existing)) {
				throw new TypeError(
					`Distributed client outputs must not overlap: \`${existing}\` and \`${out}\``
				);
			}
		}
		outputs.push(out);
		return Object.freeze({
			module: client.module,
			manifest: client.manifest,
			selector,
			documents: Object.freeze(documents),
			routes: Object.freeze(routes),
			out,
			entry: join(out, GENERATED_SVELTEKIT_MODULE),
			watchRoots: Object.freeze(documentWatchRoots(cwd, documents, out))
		});
	});
	return Object.freeze({
		cwd,
		command,
		commandArgs: Object.freeze(
			(options.commandArgs ?? []).map((argument, index) => {
				if (typeof argument !== 'string') {
					throw new TypeError(
						`Distributed commandArgs[${index}] must be a string`
					);
				}
				return argument;
			})
		),
		clients: Object.freeze(clients)
	});
}

function validateManifestSource(
	module: string,
	source: DistributedSvelteKitManifestSource
): void {
	if (typeof source === 'string') {
		nonempty(source, `${module} manifest path`);
		return;
	}
	if (
		source === null ||
		typeof source !== 'object' ||
		!Array.isArray(source.args) ||
		source.args.length === 0 ||
		source.args[0] !== 'client-manifest' ||
		source.args.some((argument) => typeof argument !== 'string')
	) {
		throw new TypeError(
			`Distributed client \`${module}\` manifest args must begin with client-manifest`
		);
	}
}

function nonempty(value: unknown, label: string): string {
	if (typeof value !== 'string' || value.trim().length === 0) {
		throw new TypeError(`${label} must be a non-empty string`);
	}
	return value;
}

function containedPath(root: string, value: string, label: string): string {
	const target = resolve(root, nonempty(value, label));
	if (!isWithin(root, target)) {
		throw new TypeError(`${label} must stay within project root ${root}`);
	}
	return target;
}

function isWithin(root: string, target: string): boolean {
	const rel = relative(root, target);
	return (
		rel === '' ||
		(!rel.startsWith(`..${sep}`) && rel !== '..' && !isAbsolute(rel))
	);
}

function documentWatchRoots(
	cwd: string,
	documents: readonly string[],
	out: string
): string[] {
	const roots = new Set<string>();
	for (const pattern of documents) {
		const normalized = pattern.replaceAll('\\', '/');
		const wildcard = normalized.search(/[*?[\]{}]/);
		const prefix = wildcard === -1 ? normalized : normalized.slice(0, wildcard);
		const base = prefix.endsWith('/')
			? prefix.slice(0, -1)
			: dirname(prefix);
		const watch = resolve(cwd, base === '.' || base.length === 0 ? '.' : base);
		if (!isWithin(cwd, watch)) {
			throw new TypeError(
				`Distributed client document glob \`${pattern}\` escapes project root ${cwd}`
			);
		}
		if (!isWithin(out, watch)) roots.add(watch);
	}
	return [...roots].sort();
}

function isClientDocumentInput(
	file: string,
	integration: ResolvedIntegration
): boolean {
	const absolute = resolve(integration.cwd, file);
	const isGraphql =
		absolute.endsWith('.graphql') || absolute.endsWith('.gql');
	const isQuerySpec = absolute.endsWith('.query.json');
	if ((!isGraphql && !isQuerySpec) || !isWithin(integration.cwd, absolute)) {
		return false;
	}
	if (
		integration.clients.some((client) => isWithin(client.out, absolute))
	) {
		return false;
	}
	return integration.clients.some((client) =>
		client.watchRoots.some((root) => isWithin(root, absolute))
	);
}

async function compileTransaction(
	integration: ResolvedIntegration,
	children: Set<ChildProcess>,
	signal: AbortSignal
): Promise<void> {
	throwIfAborted(signal);
	await validateResolvedPaths(integration);
	const transaction = await mkdtemp(
		join(integration.cwd, '.distributed-sveltekit-')
	);
	const staged: Array<{
		client: ResolvedClient;
		output: string;
		backup: string;
		hadOutput: boolean;
	}> = [];
	try {
		for (const [index, client] of integration.clients.entries()) {
			throwIfAborted(signal);
			const manifest = await materializeManifest(
				integration,
				client,
				transaction,
				index,
				children,
				signal
			);
			const output = join(transaction, `output-${index}`);
			let hadOutput = false;
			try {
				const metadata = await lstat(client.out);
				if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
					throw new Error(
						`generated output ${client.out} must be a real directory`
					);
				}
				hadOutput = true;
				await cp(client.out, output, {
					recursive: true,
					errorOnExist: true,
					force: false,
					dereference: false
				});
			} catch (error) {
				if (!isMissing(error)) throw error;
			}
			const args = [
				'client',
				'--manifest',
				manifest,
				client.selector[0],
				client.selector[1],
				...client.documents.flatMap((document) => [
					'--documents',
					document
				]),
				...client.routes.flatMap((route) => ['--route', route]),
				'--out',
				output
			];
			await runCommand(integration, args, children, signal);
			await validateGeneratedEntrypoint(integration.cwd, output, client.module);
			staged.push({
				client,
				output,
				backup: join(transaction, `backup-${index}`),
				hadOutput
			});
		}
		throwIfAborted(signal);
		await commitOutputs(integration, staged, signal);
	} finally {
		await rm(transaction, { recursive: true, force: true });
	}
}

async function checkTransaction(
	integration: ResolvedIntegration,
	children: Set<ChildProcess>,
	signal: AbortSignal
): Promise<void> {
	throwIfAborted(signal);
	await validateResolvedPaths(integration);
	const transaction = await mkdtemp(
		join(integration.cwd, '.distributed-sveltekit-check-')
	);
	try {
		for (const [index, client] of integration.clients.entries()) {
			throwIfAborted(signal);
			const manifest = await materializeManifest(
				integration,
				client,
				transaction,
				index,
				children,
				signal
			);
			await runCommand(
				integration,
				[
					'client',
					'--check',
					'--manifest',
					manifest,
					client.selector[0],
					client.selector[1],
					...client.documents.flatMap((document) => [
						'--documents',
						document
					]),
					...client.routes.flatMap((route) => ['--route', route]),
					'--out',
					client.out
				],
				children,
				signal
			);
		}
	} finally {
		await rm(transaction, { recursive: true, force: true });
	}
}

async function materializeManifest(
	integration: ResolvedIntegration,
	client: ResolvedClient,
	transaction: string,
	index: number,
	children: Set<ChildProcess>,
	signal: AbortSignal
): Promise<string> {
	throwIfAborted(signal);
	if (typeof client.manifest === 'string') {
		const path = containedPath(
			integration.cwd,
			client.manifest,
			`${client.module} manifest`
		);
		const canonicalRoot = await realpath(integration.cwd);
		const canonical = await realpath(path);
		if (!isWithin(canonicalRoot, canonical)) {
			throw new Error(
				`Distributed manifest ${path} resolves outside project root ${integration.cwd}`
			);
		}
		JSON.parse(await readFile(canonical, 'utf8')) as unknown;
		return canonical;
	}
	const result = await runCommand(
		integration,
		[...client.manifest.args],
		children,
		signal
	);
	try {
		JSON.parse(result.stdout) as unknown;
	} catch (error) {
		throw new Error(
			`dctl client-manifest for ${client.module} did not emit valid JSON`,
			{ cause: error }
		);
	}
	const temporary = join(transaction, `manifest-${index}.json.tmp`);
	const manifest = join(transaction, `manifest-${index}.json`);
	await writeFile(temporary, result.stdout, {
		encoding: 'utf8',
		flag: 'wx'
	});
	await rename(temporary, manifest);
	return manifest;
}

async function validateGeneratedEntrypoint(
	cwd: string,
	output: string,
	module: string
): Promise<void> {
	const root = await realpath(cwd);
	const entry = join(output, GENERATED_SVELTEKIT_MODULE);
	const metadata = await lstat(entry);
	if (metadata.isSymbolicLink() || !metadata.isFile()) {
		throw new Error(
			`dctl client for ${module} did not emit a regular ${GENERATED_SVELTEKIT_MODULE}`
		);
	}
	const canonical = await realpath(entry);
	if (!isWithin(root, canonical)) {
		throw new Error(
			`dctl client entrypoint ${canonical} escaped project root ${root}`
		);
	}
}

async function commitOutputs(
	integration: ResolvedIntegration,
	staged: readonly {
		client: ResolvedClient;
		output: string;
		backup: string;
		hadOutput: boolean;
	}[],
	signal: AbortSignal
): Promise<void> {
	throwIfAborted(signal);
	await validateResolvedPaths(integration);
	const applied: typeof staged[number][] = [];
	try {
		for (const item of staged) {
			throwIfAborted(signal);
			await mkdir(dirname(item.client.out), { recursive: true });
			await validateNearestExistingParent(integration.cwd, item.client.out);
			if (item.hadOutput) await rename(item.client.out, item.backup);
			try {
				await rename(item.output, item.client.out);
			} catch (error) {
				if (item.hadOutput) await rename(item.backup, item.client.out);
				throw error;
			}
			applied.push(item);
		}
		throwIfAborted(signal);
	} catch (error) {
		for (const item of [...applied].reverse()) {
			await rm(item.client.out, { recursive: true, force: true });
			if (item.hadOutput) await rename(item.backup, item.client.out);
		}
		throw error;
	}
}

async function runCommand(
	integration: ResolvedIntegration,
	args: readonly string[],
	children: Set<ChildProcess>,
	signal: AbortSignal
): Promise<{ stdout: string; stderr: string }> {
	throwIfAborted(signal);
	const argv = [...integration.commandArgs, ...args];
	return await new Promise((resolvePromise, rejectPromise) => {
		const child = spawn(integration.command, argv, {
			cwd: integration.cwd,
			env: process.env,
			shell: false,
			detached: process.platform !== 'win32',
			stdio: ['ignore', 'pipe', 'pipe']
		});
		children.add(child);
		const stdout: Buffer[] = [];
		const stderr: Buffer[] = [];
		let bytes = 0;
		let overflow = false;
		let forceKill: ReturnType<typeof setTimeout> | undefined;
		const cancel = (): void => {
			killChild(child, 'SIGTERM');
			forceKill ??= setTimeout(() => killChild(child, 'SIGKILL'), 1_500);
			forceKill.unref();
		};
		signal.addEventListener('abort', cancel, { once: true });
		const collect = (target: Buffer[]) => (chunk: Buffer | string): void => {
			if (overflow) return;
			const value = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
			bytes += value.length;
			if (bytes > MAX_COMMAND_OUTPUT_BYTES) {
				overflow = true;
				cancel();
				return;
			}
			target.push(value);
		};
		child.stdout?.on('data', collect(stdout));
		child.stderr?.on('data', collect(stderr));
		child.once('error', (error) => {
			children.delete(child);
			signal.removeEventListener('abort', cancel);
			if (forceKill !== undefined) clearTimeout(forceKill);
			rejectPromise(
				commandFailure(integration, argv, '', error.message)
			);
		});
		child.once('close', (code, signal) => {
			children.delete(child);
			cancellationCleanup();
			const out = Buffer.concat(stdout).toString('utf8');
			const err = Buffer.concat(stderr).toString('utf8');
			if (overflow) {
				rejectPromise(
					commandFailure(
						integration,
						argv,
						err,
						`output exceeded ${MAX_COMMAND_OUTPUT_BYTES} bytes`
					)
				);
			} else if (code !== 0) {
				rejectPromise(
					commandFailure(
						integration,
						argv,
						err,
						`exit ${code ?? 'unknown'}${signal === null ? '' : ` (${signal})`}`
					)
				);
			} else {
				resolvePromise({ stdout: out, stderr: err });
			}
		});

		function cancellationCleanup(): void {
			signal.removeEventListener('abort', cancel);
			if (forceKill !== undefined) clearTimeout(forceKill);
		}
	});
}

function commandFailure(
	integration: ResolvedIntegration,
	argv: readonly string[],
	stderr: string,
	summary: string
): Error {
	const renderedArgv = JSON.stringify([integration.command, ...argv]);
	const details = [
		`Distributed compiler command failed: ${summary}`,
		`cwd: ${integration.cwd}`,
		`argv: ${renderedArgv}`,
		stderr.trim().length === 0 ? undefined : `stderr:\n${stderr.trim()}`
	].filter((value): value is string => value !== undefined);
	return new Error(details.join('\n'));
}

async function validateResolvedPaths(
	integration: ResolvedIntegration
): Promise<void> {
	validateResolvedPathsSync(integration);
}

function validateResolvedPathsSync(integration: ResolvedIntegration): void {
	const canonicalRoot = realpathSync(integration.cwd);
	const physicalOutputs: Array<{ module: string; path: string }> = [];
	for (const client of integration.clients) {
		const output = plannedPhysicalPath(canonicalRoot, integration.cwd, client.out);
		for (const existing of physicalOutputs) {
			if (
				physicalPathKey(output).startsWith(
					`${physicalPathKey(existing.path)}${sep}`
				) ||
				physicalPathKey(existing.path).startsWith(
					`${physicalPathKey(output)}${sep}`
				) ||
				physicalPathKey(existing.path) === physicalPathKey(output)
			) {
				throw new Error(
					`Distributed client outputs physically overlap: ${existing.module} (${existing.path}) and ${client.module} (${output})`
				);
			}
		}
		physicalOutputs.push({ module: client.module, path: output });
		for (const watchRoot of client.watchRoots) {
			plannedPhysicalPath(canonicalRoot, integration.cwd, watchRoot);
		}
	}
}

function plannedPhysicalPath(
	canonicalRoot: string,
	lexicalRoot: string,
	target: string
): string {
	const rel = relative(lexicalRoot, target);
	let lexical = lexicalRoot;
	if (rel !== '') {
		for (const component of rel.split(sep)) {
			lexical = join(lexical, component);
			if (!existsSync(lexical)) break;
			const metadata = lstatSync(lexical);
			if (metadata.isSymbolicLink()) {
				throw new Error(
					`Distributed path ${target} contains symlink component ${lexical}`
				);
			}
		}
	}

	let existing = target;
	const suffix: string[] = [];
	while (!existsSync(existing)) {
		const parent = dirname(existing);
		if (parent === existing) {
			throw new Error(
				`Distributed path ${target} has no existing parent within ${canonicalRoot}`
			);
		}
		suffix.unshift(basename(existing));
		existing = parent;
	}
	const canonicalExisting = realpathSync(existing);
	if (!isWithin(canonicalRoot, canonicalExisting)) {
		throw new Error(
			`Distributed path ${target} resolves outside project root ${canonicalRoot}`
		);
	}
	const planned = resolve(canonicalExisting, ...suffix);
	if (!isWithin(canonicalRoot, planned)) {
		throw new Error(
			`Distributed path ${target} resolves outside project root ${canonicalRoot}`
		);
	}
	return planned;
}

function physicalPathKey(path: string): string {
	return process.platform === 'win32' || process.platform === 'darwin'
		? path.toLocaleLowerCase('en-US')
		: path;
}

async function validateNearestExistingParent(
	root: string,
	target: string
): Promise<void> {
	const canonicalRoot = realpathSync(root);
	plannedPhysicalPath(canonicalRoot, root, target);
}

function compilerCoordinatorRegistry(): Map<string, CompilerCoordinator> {
	const globalScope = globalThis as unknown as Record<symbol, unknown>;
	const existing = globalScope[COMPILER_COORDINATORS];
	if (existing instanceof Map) {
		return existing as Map<string, CompilerCoordinator>;
	}
	const registry = new Map<string, CompilerCoordinator>();
	globalScope[COMPILER_COORDINATORS] = registry;
	return registry;
}

async function acquireCompilerLock(cwd: string): Promise<CompilerLockLease> {
	const path = join(cwd, COMPILER_LOCK);
	const registry = compilerCoordinatorRegistry();
	for (;;) {
		const existing = registry.get(path);
		if (existing?.closing !== undefined) {
			await existing.closing;
			continue;
		}
		const coordinator =
			existing ??
			({
				path,
				references: 0,
				ready: acquirePhysicalCompilerLock(cwd, path),
				tail: Promise.resolve(),
				startupRuns: new Map()
			} satisfies CompilerCoordinator);
		if (existing === undefined) registry.set(path, coordinator);
		coordinator.references += 1;
		try {
			await coordinator.ready;
			return { coordinator, released: false };
		} catch (error) {
			coordinator.references -= 1;
			if (
				coordinator.references === 0 &&
				registry.get(path) === coordinator
			) {
				registry.delete(path);
			}
			throw error;
		}
	}
}

async function acquirePhysicalCompilerLock(
	cwd: string,
	path: string
): Promise<void> {
	await validateNearestExistingParent(cwd, dirname(path));
	await mkdir(dirname(path), { recursive: true });
	await validateNearestExistingParent(cwd, path);
	for (let attempt = 0; attempt < 2; attempt += 1) {
		try {
			const handle = await open(path, 'wx', 0o600);
			try {
				await handle.writeFile(
					`${JSON.stringify({ pid: process.pid, version: 1 })}\n`,
					'utf8'
				);
			} finally {
				await handle.close();
			}
			return;
		} catch (error) {
			if (!isAlreadyExists(error)) throw error;
			let owner: unknown;
			try {
				owner = JSON.parse(await readFile(path, 'utf8')) as unknown;
			} catch {
				owner = undefined;
			}
			const pid =
				owner !== null &&
				typeof owner === 'object' &&
				'pid' in owner &&
				typeof owner.pid === 'number'
					? owner.pid
					: undefined;
			if (pid !== undefined && processExists(pid)) {
				throw new Error(
					`Distributed SvelteKit compiler already owns ${cwd} (pid ${pid})`
				);
			}
			await unlink(path).catch((unlinkError: unknown) => {
				if (!isMissing(unlinkError)) throw unlinkError;
			});
		}
	}
	throw new Error(`could not acquire Distributed SvelteKit compiler lock ${path}`);
}

function withCompilerLock<T>(
	lease: CompilerLockLease,
	operation: () => Promise<T>
): Promise<T> {
	if (lease.released) {
		return Promise.reject(
			new Error('Distributed SvelteKit compiler lock lease is already released')
		);
	}
	const coordinator = lease.coordinator;
	const run = coordinator.tail.catch(() => undefined).then(operation);
	coordinator.tail = run.then(
		() => undefined,
		() => undefined
	);
	return run;
}

function withCompilerStartup(
	lease: CompilerLockLease,
	integration: ResolvedIntegration,
	operation: () => Promise<void>
): Promise<void> {
	if (lease.released) {
		return Promise.reject(
			new Error('Distributed SvelteKit compiler lock lease is already released')
		);
	}
	const key = JSON.stringify(integration);
	const coordinator = lease.coordinator;
	const existing = coordinator.startupRuns.get(key);
	if (existing !== undefined) return existing;
	const run = withCompilerLock(lease, operation);
	const cached = run.catch((error: unknown) => {
		if (coordinator.startupRuns.get(key) === cached) {
			coordinator.startupRuns.delete(key);
		}
		throw error;
	});
	coordinator.startupRuns.set(key, cached);
	return cached;
}

function requireCompilerLock(
	lease: CompilerLockLease | undefined
): CompilerLockLease {
	if (lease === undefined) {
		throw new Error('Distributed SvelteKit compiler lock is not initialized');
	}
	return lease;
}

async function releaseCompilerLock(lease: CompilerLockLease): Promise<void> {
	if (lease.released) return;
	lease.released = true;
	const coordinator = lease.coordinator;
	await coordinator.tail.catch(() => undefined);
	coordinator.references -= 1;
	if (coordinator.references > 0) return;
	if (coordinator.references < 0) {
		throw new Error('Distributed SvelteKit compiler lock reference underflow');
	}
	const registry = compilerCoordinatorRegistry();
	if (registry.get(coordinator.path) !== coordinator) return;
	const closing = unlink(coordinator.path)
		.catch((error: unknown) => {
			if (!isMissing(error)) throw error;
		})
		.finally(() => {
			if (registry.get(coordinator.path) === coordinator) {
				registry.delete(coordinator.path);
			}
		});
	coordinator.closing = closing;
	await closing;
}

function processExists(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch (error) {
		return (
			error !== null &&
			typeof error === 'object' &&
			'code' in error &&
			error.code === 'EPERM'
		);
	}
}

function killChild(
	child: ChildProcess,
	signal: NodeJS.Signals
): void {
	if (child.pid === undefined) return;
	if (process.platform !== 'win32') {
		try {
			process.kill(-child.pid, signal);
			return;
		} catch {
			// The child may have exited or failed to become its own process group.
		}
	}
	child.kill(signal);
}

function throwIfAborted(signal: AbortSignal): void {
	if (!signal.aborted) return;
	const error = new Error('Distributed SvelteKit compiler was cancelled');
	error.name = 'AbortError';
	throw error;
}

function isMissing(error: unknown): boolean {
	return (
		error !== null &&
		typeof error === 'object' &&
		'code' in error &&
		error.code === 'ENOENT'
	);
}

function isAlreadyExists(error: unknown): boolean {
	return (
		error !== null &&
		typeof error === 'object' &&
		'code' in error &&
		error.code === 'EEXIST'
	);
}

function requireResolved(
	value: ResolvedIntegration | undefined
): ResolvedIntegration {
	if (value === undefined) {
		throw new Error(
			'Distributed SvelteKit Vite plugin has not received resolved config'
		);
	}
	return value;
}

function virtualId(module: string): string {
	return `\0@hops-ops/distributed:sveltekit:${module}`;
}

function portablePath(path: string): string {
	return path.replaceAll('\\', '/');
}

function viteError(error: unknown): Readonly<{
	message: string;
	stack?: string;
}> {
	if (error instanceof Error) {
		return {
			message: error.message,
			...(error.stack === undefined ? {} : { stack: error.stack })
		};
	}
	return { message: String(error) };
}
