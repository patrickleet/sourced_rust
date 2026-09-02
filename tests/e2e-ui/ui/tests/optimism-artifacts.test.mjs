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
import vm from 'node:vm';

import { generatedPath } from './lifecycle-generation.mjs';

const commandsPath = generatedPath('user', 'commands.ts');

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
		demo: 'todos',
		previewValues: { assignee_id: null, status: 'open' }
	},
	{
		name: 'todo.complete',
		mutationField: 'todos_complete',
		consistency: 'eventual',
		demo: 'todos',
		previewValues: { status: 'completed' }
	},
	{
		name: 'todo.reopen',
		mutationField: 'todos_reopen',
		consistency: 'eventual',
		demo: 'todos',
		previewValues: { status: 'open' }
	},
	{
		name: 'todo.archive',
		mutationField: 'todos_archive',
		consistency: 'eventual',
		demo: 'todos',
		previewValues: { status: 'archived' }
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
		demo: 'blob'
		// Thin input (game_id + direction); board seals via Atomic response.
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

		for (const [field, expectedValue] of Object.entries(
			expected.previewValues ?? {}
		)) {
			const assignment = previewOps
				.flatMap(({ mutation }) => mutation.fields ?? mutation.set ?? [])
				.find((candidate) => candidate.field === field);
			assert.ok(assignment, `${expected.name} preview must assign ${field}`);
			if (expectedValue === null) {
				assert.equal(assignment.value.kind, 'null');
			} else {
				assert.equal(assignment.value.kind, 'constant');
				assert.equal(assignment.value.value.type, 'string');
				assert.equal(assignment.value.value.value, expectedValue);
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
