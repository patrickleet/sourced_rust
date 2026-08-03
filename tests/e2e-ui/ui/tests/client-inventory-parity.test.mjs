import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { validateClientInventory } from '../distributed.config.js';

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
