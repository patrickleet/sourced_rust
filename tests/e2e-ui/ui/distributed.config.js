import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import clientInventory from './distributed.clients.json' with { type: 'json' };

const uiRoot = dirname(fileURLToPath(import.meta.url));
const e2eRoot = resolve(uiRoot, '..');
const distributedRoot = resolve(uiRoot, '../../..');

const CLIENT_INVENTORY_SCHEMA_VERSION = 1;
const CLIENT_MODULE = /^\$distributed(?:\/[A-Za-z0-9][A-Za-z0-9._-]*)*$/;

function assertPortablePath(value, label, { allowGlob = false } = {}) {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value !== value.trim() ||
		value.includes('\0') ||
		value.includes('\\') ||
		/(?:postgres(?:ql)?:\/\/|password=|token=|secret=|bearer )/i.test(value) ||
		value.startsWith('/') ||
		value.startsWith('~') ||
		value.split('/').some((part) => part === '..' || part === '.') ||
		(!allowGlob && /[*?\[\]{]/.test(value))
	) {
		throw new TypeError(`${label} must be a portable repository-relative path`);
	}
}

function validateClientInventory(value) {
	if (
		value === null ||
		typeof value !== 'object' ||
		value.schema_version !== CLIENT_INVENTORY_SCHEMA_VERSION ||
		!Array.isArray(value.clients) ||
		value.clients.length === 0 ||
		value.clients.length > 64
	) {
		throw new TypeError(
			`distributed.clients.json must be schema version ${CLIENT_INVENTORY_SCHEMA_VERSION} with 1..=64 clients`
		);
	}
	const modules = new Set();
	const surfaces = new Set();
	const outputs = new Set();
	const allowedKeys = new Set([
		'module',
		'surface',
		'documents',
		'output',
		'manifest_entrypoint'
	]);
	return Object.freeze(
		value.clients.map((client, index) => {
			if (client === null || typeof client !== 'object') {
				throw new TypeError(`distributed client declaration ${index} must be an object`);
			}
			for (const key of Object.keys(client)) {
				if (!allowedKeys.has(key)) {
					throw new TypeError(`distributed client ${index} contains unsupported field ${key}`);
				}
			}
			if (typeof client.module !== 'string' || !CLIENT_MODULE.test(client.module)) {
				throw new TypeError(`distributed client ${index} has an invalid module`);
			}
			if (
				typeof client.surface !== 'string' ||
				!/^[A-Za-z0-9._:/-]+$/.test(client.surface) ||
				client.surface.trim().length === 0
			) {
				throw new TypeError(`distributed client ${client.module} has an invalid surface`);
			}
			if (!Array.isArray(client.documents) || client.documents.length === 0 || client.documents.length > 64) {
				throw new TypeError(`distributed client ${client.module} must declare 1..=64 documents`);
			}
			const documents = client.documents.map((document, documentIndex) => {
				assertPortablePath(document, `${client.module} documents[${documentIndex}]`, { allowGlob: true });
				if (!document.endsWith('.graphql') && !document.endsWith('.gql')) {
					throw new TypeError(`${client.module} documents[${documentIndex}] must end in .graphql or .gql`);
				}
				if (document.includes('**') || /^[*?\[\]{]/.test(document)) {
					throw new TypeError(`${client.module} document glob is unbounded`);
				}
				return document;
			});
			assertPortablePath(client.output, `${client.module} output`);
			const manifestEntrypoint = client.manifest_entrypoint ?? undefined;
			if (typeof manifestEntrypoint === 'string') {
				if (
					manifestEntrypoint.length === 0 ||
					manifestEntrypoint.split('::').some(
						(segment) => !/^[A-Za-z0-9_]+$/.test(segment)
					)
				) {
					throw new TypeError(`${client.module} manifest_entrypoint is invalid`);
				}
			} else if (client.manifest_entrypoint !== undefined && client.manifest_entrypoint !== null) {
				throw new TypeError(`${client.module} manifest_entrypoint must be a string`);
			}
			if (!modules.add(client.module)) throw new TypeError(`duplicate client module ${client.module}`);
			if (!surfaces.add(client.surface)) throw new TypeError(`duplicate client surface ${client.surface}`);
			if (!outputs.add(client.output)) throw new TypeError(`duplicate client output ${client.output}`);
			return Object.freeze({
				module: client.module,
				surface: client.surface,
				documents: Object.freeze(documents),
				output: client.output,
				manifest_entrypoint: manifestEntrypoint
			});
		})
	);
}

const clientDeclarations = validateClientInventory(clientInventory);

const manifestArgs = [
	'client-manifest',
	'--manifest-path',
	resolve(e2eRoot, 'Cargo.toml'),
	'--package',
	'e2e-service',
	'--distributed-path',
	distributedRoot
];
/** App-owned declarations come from distributed.clients.json; this file adds executable and local paths. */
export const distributedClients = Object.freeze(
	clientDeclarations.map((client) =>
		Object.freeze({
			module: client.module,
			manifest: Object.freeze({
				args: Object.freeze([
					...manifestArgs,
					...(client.manifest_entrypoint === undefined
						? []
						: ['--entrypoint', client.manifest_entrypoint])
				])
			}),
			surface: client.surface,
			documents: client.documents,
			out: client.output
		})
	)
);

export const distributedViteOptions = Object.freeze({
	cwd: uiRoot,
	command: 'cargo',
	commandArgs: Object.freeze([
		'run',
		'--quiet',
		'--manifest-path',
		resolve(distributedRoot, 'Cargo.toml'),
		'-p',
		'distributed_cli',
		'--bin',
		'dctl',
		'--'
	]),
	clients: distributedClients
});
