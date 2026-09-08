import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createDistributedReplica } from '../dist/replica/index.js';
import { ControlledReplicaTransport } from './fixtures/adapter-conformance.mjs';

const artifact = JSON.parse(readFileSync(new URL(
	'../../distributed_cli/tests/fixtures/unique-key-bridge-operation.json', import.meta.url
), 'utf8'));
const root = artifact.roots[0];
const owner = root.selection.members.find(member => member.field === 'owner');

function frame(position, targetId, targetTitle) {
	const records = [];
	function wire(selection, values, path) {
		records.push({ path, model: selection.storage.model,
			scopeToken: `record:${values.id}`, incarnation: '1', revision: position, tombstone: false });
		return Object.fromEntries(selection.members.map(member => {
			if (member.kind === 'branch') return [member.responseKey, targetId === null ? null :
				wire(member.selection, { id: targetId, title: targetTitle, tenantId: 'tenant-a',
					__typename: 'todo' }, [...path, member.responseKey])];
			assert.ok(Object.hasOwn(values, member.field), member.field);
			return [member.responseKey, values[member.field]];
		}));
	}
	const row = wire(root.selection, { id: 'source-id', title: 'source',
		ownerTitle: targetTitle, tenantId: 'tenant-a', __typename: 'todo' }, ['todos', '0']);
	return { data: { todos: [row] }, extensions: { distributed: {
		protocolVersion: artifact.protocol.version, schemaHash: artifact.protocol.schemaHash,
		authorizationGeneration: 'auth-1', cacheScope: 'unique-key-cache',
		operation: position === '1' ? artifact.id : artifact.live.id,
		...(position === '1' ? {} : { live: { supported: true, reset: false, cursors: [
			{ projection: 'unique-key-projector', position, token: `resume:${position}` }
		] } }),
		snapshot: { scopeToken: 'unique-key-snapshot', recordsComplete: true, indexesComparable: true,
			records, indexes: [{ projection: 'unique-key-projector', scopeToken: 'unique-key-index', position,
				resume: { projection: 'unique-key-projector', position, token: `resume:${position}` } }], observations: [] }
	} } };
}

test('Rust-generated candidate-key relationship survives hydration and live reference changes', async () => {
	assert.deepEqual(owner.relationship.keyMapping, {
		kind: 'direct', local: ['tenantId', 'ownerTitle'], remote: ['tenantId', 'title']
	});
	assert.deepEqual(owner.selection.storage.identityFields, ['tenantId', 'id']);
	const transport = new ControlledReplicaTransport();
	const server = createDistributedReplica({ transport });
	const watch = server.watch(artifact, {}, { live: false });
	const pending = watch.refresh();
	await Promise.resolve();
	transport.fetches[0].response.resolve(frame('1', 'target-one', 'first'));
	await pending;
	assert.deepEqual(server.read(artifact, {}).data.todos[0], {
		id: 'source-id', title: 'source', owner: { id: 'target-one', title: 'first' }
	});
	watch.destroy();
	const seed = server.dehydrate();
	const browserTransport = new ControlledReplicaTransport();
	const browser = createDistributedReplica({ transport: browserTransport });
	assert.equal(browser.hydrate(seed, seed.scope), true);
	assert.deepEqual(browser.read(artifact, {}).data, server.read(artifact, {}).data);
	const live = browser.watch(artifact, {}, { live: true });
	const unsubscribe = live.subscribe(() => {});
	assert.equal(browserTransport.fetches.length, 0);
	assert.equal(browserTransport.lives.length, 1);
	browserTransport.lives[0].observer.next(frame('2', 'target-two', 'second'));
	assert.deepEqual(live.get().data.todos[0], {
		id: 'source-id', title: 'source', owner: { id: 'target-two', title: 'second' }
	});
	browserTransport.lives[0].observer.next(frame('3', null, null));
	assert.equal(live.get().data.todos[0].owner, null);
	assert.equal(live.get().data.todos[0].id, 'source-id');
	unsubscribe();
	live.destroy();
	assert.equal(browserTransport.lives[0].closed, true);
});
