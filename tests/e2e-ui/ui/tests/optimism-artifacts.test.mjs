/**
 * Offline gate: every demo write command must export client-side optimism IR.
 *
 * - Eventual: non-empty projection.preview.operations
 * - Atomic: directProjection + non-empty projection.preview.operations
 *   (same mutation IR; apply site differs)
 *
 * This does not prove the UI paints before the wire — that is Playwright —
 * but it fails closed when gen/preview eligibility regresses.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const commandsPath = path.join(uiRoot, 'src/lib/generated/user/commands.ts');

/**
 * Extract frozen command artifact objects from the generated TS module without
 * importing the replica package (keeps this test offline and dependency-light).
 */
function loadCommandArtifacts() {
	const source = fs.readFileSync(commandsPath, 'utf8');
	// Each artifact is `export const Command_… = { … };`
	const artifacts = [];
	const re =
		/export const (Command_[A-Za-z0-9_]+): ReplicaCommandArtifact[^=]*=\s*(\{[\s\S]*?\n\});/g;
	let match;
	while ((match = re.exec(source)) !== null) {
		const name = match[1];
		const jsonish = match[2]
			// Allow trailing commas that TS permits
			.replace(/,(\s*[}\]])/g, '$1');
		let value;
		try {
			value = vm.runInNewContext(`(${jsonish})`);
		} catch (error) {
			throw new Error(`failed to parse ${name}: ${error}`);
		}
		artifacts.push({ exportName: name, ...value });
	}
	assert.ok(artifacts.length > 0, 'expected generated command artifacts');
	return artifacts;
}

const DEMO_COMMANDS = Object.freeze([
	{
		name: 'chat.post',
		mutationField: 'chat_messages_post',
		consistency: 'eventual',
		demo: 'chat'
	},
	{
		name: 'todo.create',
		mutationField: 'todos_create',
		consistency: 'eventual',
		demo: 'todos'
	},
	{
		name: 'todo.complete',
		mutationField: 'todos_complete',
		consistency: 'eventual',
		demo: 'todos'
	},
	{
		name: 'todo.reopen',
		mutationField: 'todos_reopen',
		consistency: 'eventual',
		demo: 'todos'
	},
	{
		name: 'todo.archive',
		mutationField: 'todos_archive',
		consistency: 'eventual',
		demo: 'todos'
	},
	{
		name: 'todo.rename',
		mutationField: 'todos_rename',
		consistency: 'eventual',
		demo: 'todos'
	},
	{
		name: 'todo.purge',
		mutationField: 'todos_purge',
		consistency: 'eventual',
		demo: 'todos'
	},
	{
		name: 'blob.start',
		mutationField: 'blob_games_start',
		consistency: 'atomic',
		demo: 'blob'
	},
	{
		name: 'blob.move',
		mutationField: 'blob_games_move',
		consistency: 'atomic',
		demo: 'blob',
		// Board fields must be input-backed for paint-before-wire.
		previewFields: [
			'map_json',
			'score',
			'player_dead',
			'status',
			'current_level',
			'current_level_completed'
		]
	},
	{
		name: 'blob.start_level',
		mutationField: 'blob_games_start_level',
		consistency: 'atomic',
		demo: 'blob'
	}
]);

test('demo commands export preview IR for client optimism', () => {
	const artifacts = loadCommandArtifacts();
	const byField = new Map(artifacts.map((a) => [a.mutationField, a]));

	for (const expected of DEMO_COMMANDS) {
		const artifact = byField.get(expected.mutationField);
		assert.ok(
			artifact,
			`missing generated command for ${expected.name} (${expected.mutationField})`
		);
		assert.equal(
			artifact.consistency,
			expected.consistency,
			`${expected.name} consistency`
		);
		assert.equal(artifact.name, expected.name, `${expected.name} command name`);

		const previewOps = artifact.projection?.preview?.operations ?? [];
		assert.ok(
			previewOps.length > 0,
			`${expected.name} (${expected.demo}) must export non-empty projection.preview.operations for client optimism`
		);

		if (expected.previewFields?.length) {
			const previewJson = JSON.stringify(artifact.projection?.preview ?? {});
			for (const field of expected.previewFields) {
				assert.ok(
					previewJson.includes(`"field":"${field}"`) ||
						previewJson.includes(`"field": "${field}"`),
					`${expected.name} preview must include field ${field}`
				);
			}
		}

		if (expected.consistency === 'atomic') {
			assert.ok(
				artifact.directProjection?.model,
				`${expected.name} atomic commands must export directProjection`
			);
		} else {
			assert.equal(
				artifact.directProjection,
				undefined,
				`${expected.name} eventual commands must not export directProjection`
			);
		}
	}
});
