import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...segments) =>
	fs.readFileSync(path.join(uiRoot, ...segments), 'utf8');

test('normal and elevated command inventories cannot be mixed', () => {
	const user = read('src/lib/generated/user/commands.ts');
	const admin = read('src/lib/generated/admin/commands.ts');

	assert.match(user, /"name": "todo\.create"/);
	assert.match(user, /"name": "chat\.post"/);
	assert.match(user, /"name": "blob\.start"/);
	assert.match(user, /"name": "blob\.move"/);
	assert.doesNotMatch(user, /"name": "todo\.force_archive"/);
	assert.match(user, /"kind": "application"/);
	assert.match(user, /"name": "e2e-ui"/);
	assert.match(user, /"roles": \[\s*"admin",\s*"user"\s*\]/);

	assert.match(admin, /"name": "todo\.force_archive"/);
	assert.match(admin, /"name": "e2e-ui-admin"/);
	assert.match(admin, /"roles": \[\s*"admin"\s*\]/);
});
