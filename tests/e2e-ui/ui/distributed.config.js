// @ts-nocheck
// Config script for the Distributed SvelteKit plugin inventory. Checked by
// Node at load time; not part of the app type graph. `// @ts-nocheck` keeps
// `svelte-check` (checkJs) from requiring annotations on every helper.
import { dirname, resolve } from 'node:path';
import {
	closeSync,
	constants,
	fstatSync,
	lstatSync,
	openSync,
	readSync
} from 'node:fs';
import { fileURLToPath } from 'node:url';

const uiRoot = dirname(fileURLToPath(import.meta.url));
const e2eRoot = resolve(uiRoot, '..');
const distributedRoot = resolve(uiRoot, '../../..');
const clientInventoryPath = resolve(uiRoot, 'distributed.clients.json');

const CLIENT_INVENTORY_SCHEMA_VERSION = 1;
const MAX_CLIENT_INVENTORY_BYTES = 1024 * 1024;
const MAX_CLIENT_JSON_DEPTH = 24;
const MAX_CLIENT_JSON_BRACKET_DEPTH = MAX_CLIENT_JSON_DEPTH + 1;
const MAX_CLIENT_STRING_BYTES = 4 * 1024;
const CLIENT_MODULE = /^\$distributed(?:\/[A-Za-z0-9][A-Za-z0-9._-]*)*$/;
const CLIENT_INVENTORY_KEYS = new Set(['schema_version', 'clients']);
const SECRET_LIKE = /(?:postgres(?:ql)?:\/\/|mysql:\/\/|mongodb:\/\/|bearer |password=|token=|secret=|-----begin )/i;
const textEncoder = new TextEncoder();

function readBoundedInventory(filePath) {
	const preflight = lstatSync(filePath);
	if (preflight.isSymbolicLink()) {
		throw new TypeError('distributed.clients.json must not be a symlink');
	}
	if (!preflight.isFile()) {
		throw new TypeError('distributed.clients.json must be a regular file');
	}
	if (preflight.size > MAX_CLIENT_INVENTORY_BYTES) {
		throw new TypeError(
			`distributed.clients.json exceeds maximum size ${MAX_CLIENT_INVENTORY_BYTES} bytes`
		);
	}

	let descriptor;
	try {
		const noFollow = constants.O_NOFOLLOW ?? 0;
		descriptor = openSync(filePath, constants.O_RDONLY | noFollow);
	} catch (error) {
		if (error?.code === 'ELOOP') {
			throw new TypeError('distributed.clients.json must not be a symlink');
		}
		throw error;
	}
	try {
		const metadata = fstatSync(descriptor);
		if (!metadata.isFile()) {
			throw new TypeError('distributed.clients.json must be a regular file');
		}
		if (metadata.size > MAX_CLIENT_INVENTORY_BYTES) {
			throw new TypeError(
				`distributed.clients.json exceeds maximum size ${MAX_CLIENT_INVENTORY_BYTES} bytes`
			);
		}

		const buffer = Buffer.allocUnsafe(MAX_CLIENT_INVENTORY_BYTES + 1);
		let offset = 0;
		while (offset < buffer.length) {
			const bytesRead = readSync(descriptor, buffer, offset, buffer.length - offset, null);
			if (bytesRead === 0) break;
			offset += bytesRead;
		}
		if (offset > MAX_CLIENT_INVENTORY_BYTES) {
			throw new TypeError(
				`distributed.clients.json exceeds maximum size ${MAX_CLIENT_INVENTORY_BYTES} bytes`
			);
		}
		return new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(0, offset));
	} finally {
		closeSync(descriptor);
	}
}

function assertJsonNestingDepth(source) {
	let depth = 0;
	let inString = false;
	let escaped = false;
	for (const character of source) {
		if (inString) {
			if (escaped) {
				escaped = false;
			} else if (character === '\\') {
				escaped = true;
			} else if (character === '"') {
				inString = false;
			}
			continue;
		}
		if (character === '"') {
			inString = true;
		} else if (character === '{' || character === '[') {
			depth += 1;
			if (depth > MAX_CLIENT_JSON_BRACKET_DEPTH) {
				throw new TypeError(
					`distributed.clients.json exceeds maximum JSON nesting depth ${MAX_CLIENT_JSON_DEPTH}`
				);
			}
		} else if (character === '}' || character === ']') {
			depth -= 1;
		}
	}
}

function assertPortablePath(value, label, { allowGlob = false } = {}) {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value !== value.trim() ||
		textEncoder.encode(value).length > MAX_CLIENT_STRING_BYTES ||
		value.includes('\0') ||
		value.includes('\\') ||
		SECRET_LIKE.test(value) ||
		value.startsWith('/') ||
		value.startsWith('~') ||
		textEncoder.encode(value)[1] === 0x3a ||
		value.split('/').some((part) => part === '..' || part === '.') ||
		(!allowGlob && /[*?\[\]{]/.test(value))
	) {
		throw new TypeError(`${label} must be a portable repository-relative path`);
	}
}

function assertClientIdentifier(value, label) {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value !== value.trim() ||
		textEncoder.encode(value).length > MAX_CLIENT_STRING_BYTES ||
		value.includes('..') ||
		SECRET_LIKE.test(value)
	) {
		throw new TypeError(`${label} must be a portable identifier`);
	}
}

export function validateClientInventory(value) {
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
	for (const key of Object.keys(value)) {
		if (!CLIENT_INVENTORY_KEYS.has(key)) {
			throw new TypeError(`distributed.clients.json contains unsupported field ${key}`);
		}
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
			if (
				typeof client.module !== 'string' ||
				!CLIENT_MODULE.test(client.module) ||
				client.module.includes('..') ||
				textEncoder.encode(client.module).length > MAX_CLIENT_STRING_BYTES ||
				SECRET_LIKE.test(client.module)
			) {
				throw new TypeError(`distributed client ${index} has an invalid module`);
			}
			if (
				typeof client.surface !== 'string' ||
				!/^[A-Za-z0-9._:/-]+$/.test(client.surface) ||
				client.surface.trim().length === 0
			) {
				throw new TypeError(`distributed client ${client.module} has an invalid surface`);
			}
			assertClientIdentifier(client.surface, `${client.module} surface`);
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
			if (new Set(documents).size !== documents.length) {
				throw new TypeError(`distributed client ${client.module} contains duplicate documents`);
			}
			assertPortablePath(client.output, `${client.module} output`);
			const manifestEntrypoint = client.manifest_entrypoint ?? undefined;
			if (typeof manifestEntrypoint === 'string') {
				if (
					manifestEntrypoint.length === 0 ||
					textEncoder.encode(manifestEntrypoint).length > MAX_CLIENT_STRING_BYTES ||
					manifestEntrypoint.split('::').some(
						(segment) => !/^[A-Za-z0-9_]+$/.test(segment)
					)
				) {
					throw new TypeError(`${client.module} manifest_entrypoint is invalid`);
				}
			} else if (client.manifest_entrypoint !== undefined && client.manifest_entrypoint !== null) {
				throw new TypeError(`${client.module} manifest_entrypoint must be a string`);
			}
			if (modules.has(client.module)) throw new TypeError(`duplicate client module ${client.module}`);
			if (surfaces.has(client.surface)) throw new TypeError(`duplicate client surface ${client.surface}`);
			if (outputs.has(client.output)) throw new TypeError(`duplicate client output ${client.output}`);
			modules.add(client.module);
			surfaces.add(client.surface);
			outputs.add(client.output);
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

export function loadClientInventory(filePath = clientInventoryPath) {
	const source = readBoundedInventory(filePath);
	assertJsonNestingDepth(source);
	let value;
	try {
		value = JSON.parse(source);
	} catch {
		throw new TypeError('distributed.clients.json is invalid JSON; check its syntax');
	}
	return validateClientInventory(value);
}

const clientDeclarations = loadClientInventory();

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
		'distributed',
		'--'
	]),
	clients: distributedClients
});
