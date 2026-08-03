import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadClientInventory, validateClientInventory } from '../distributed.config.js';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repositoryRoot = path.resolve(uiRoot, '../../..');
const parityPath = path.join(
	repositoryRoot,
	'distributed_cli',
	'tests',
	'fixtures',
	'contracts',
	'client-inventory-parity.json'
);

test('Rust and JavaScript client inventory validators share parity vectors', () => {
	const { vectors } = JSON.parse(fs.readFileSync(parityPath, 'utf8'));
	for (const vector of vectors) {
		let valid = true;
		try {
			validateClientInventory(vector.inventory);
		} catch {
			valid = false;
		}
		assert.equal(valid, vector.valid, vector.name);
	}
});

test('Vite client inventory loading rejects a sparse oversized file before reading it', () => {
	const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'distributed-client-inventory-'));
	const inventoryPath = path.join(temporaryRoot, 'oversized.json');
	const descriptor = fs.openSync(inventoryPath, 'w');
	try {
		fs.ftruncateSync(descriptor, 1024 * 1024 + 1);
	} finally {
		fs.closeSync(descriptor);
	}
	try {
		assert.throws(() => loadClientInventory(inventoryPath), /maximum size/);
	} finally {
		fs.rmSync(temporaryRoot, { recursive: true, force: true });
	}
});

test('Vite client inventory loading rejects symlink inputs', () => {
	const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'distributed-client-inventory-'));
	const inventoryPath = path.join(temporaryRoot, 'distributed.clients.json');
	const linkPath = path.join(temporaryRoot, 'inventory-link.json');
	fs.writeFileSync(inventoryPath, fs.readFileSync(path.join(uiRoot, 'distributed.clients.json')));
	fs.symlinkSync(inventoryPath, linkPath);
	try {
		assert.throws(() => loadClientInventory(linkPath), /must not be a symlink/);
	} finally {
		fs.rmSync(temporaryRoot, { recursive: true, force: true });
	}
});

test('Vite client inventory loading rejects excessive JSON nesting before parsing', () => {
	const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'distributed-client-inventory-'));
	const inventoryPath = path.join(temporaryRoot, 'deep.json');
	const depth = 26;
	fs.writeFileSync(inventoryPath, `${'['.repeat(depth)}null${']'.repeat(depth)}`);
	try {
		assert.throws(() => loadClientInventory(inventoryPath), /nesting depth/);
	} finally {
		fs.rmSync(temporaryRoot, { recursive: true, force: true });
	}
});

test('Vite invalid JSON errors do not echo malformed secret-like source text', () => {
	const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'distributed-client-inventory-'));
	const inventoryPath = path.join(temporaryRoot, 'malformed.json');
	const secret = 'password=super-secret-value';
	fs.writeFileSync(
		inventoryPath,
		`{"schema_version":1,"clients":[{"module":"$distributed","documents":["${secret}`
	);
	try {
		assert.throws(
			() => loadClientInventory(inventoryPath),
			(error) => {
				assert.match(error.message, /invalid JSON/);
				assert.doesNotMatch(error.message, /super-secret-value/);
				assert.doesNotMatch(error.message, /password=/);
				return true;
			}
		);
	} finally {
		fs.rmSync(temporaryRoot, { recursive: true, force: true });
	}
});
