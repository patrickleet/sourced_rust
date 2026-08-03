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
