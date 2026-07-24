import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
	canonicalizeOperationVariables,
	createDistributedReplica
} from '../dist/replica/index.js';

const artifact = JSON.parse(
	readFileSync(
		new URL(
			'../../distributed_cli/tests/fixtures/runtime-bridge-operation.json',
			import.meta.url
		),
		'utf8'
	)
);

const singletonVariables = {
	id: 7,
	tenantId: 'tenant-a'
};

const expandedVariables = {
	id: '7',
	tenantId: 'tenant-a'
};

function wireRecord(root) {
	const values = {
		__typename: root.selection.typename,
		id: '7',
		tenantId: 'tenant-a',
		title: 'from the Rust artifact'
	};
	return Object.fromEntries(
		root.selection.members
			.filter((member) => member.kind === 'scalar')
			.map((member) => {
				assert.ok(
					Object.hasOwn(values, member.field),
					`bridge test needs a wire value for generated field ${member.field}`
				);
				return [member.responseKey, values[member.field]];
			})
	);
}

function protocolEnvelope(root) {
	const projection = 'runtime-bridge-projector';
	const position = '1';
	return {
		data: {
			[root.responseKey]: wireRecord(root)
		},
		extensions: {
			distributed: {
				protocolVersion: artifact.protocol.version,
				schemaHash: artifact.protocol.schemaHash,
				cacheScope: 'runtime-bridge-cache',
				operation: artifact.id,
				snapshot: {
					scopeToken: 'runtime-bridge-snapshot',
					recordsComplete: true,
					indexesComparable: true,
					records: [
						{
							path: [root.responseKey],
							model: root.selection.storage.model,
							scopeToken: 'runtime-bridge-record-7',
							incarnation: position,
							revision: position,
							tombstone: false
						}
					],
					indexes: [
						{
							projection,
							scopeToken: 'runtime-bridge-index',
							position,
							resume: {
								projection,
								position,
								token: 'runtime-bridge-resume'
							}
						}
					],
					observations: []
				}
			}
		}
	};
}

test('the JS replica executes the exact machine-readable Rust operation artifact', () => {
	assert.equal(artifact.protocol.operation, artifact.id);
	assert.equal(artifact.roots.length, 1);

	const canonicalSingleton = canonicalizeOperationVariables(
		artifact,
		singletonVariables
	);
	const canonicalExpanded = canonicalizeOperationVariables(
		artifact,
		expandedVariables
	);
	assert.deepEqual(canonicalSingleton, canonicalExpanded);

	const root = artifact.roots[0];
	const replica = createDistributedReplica();
	replica.writeResult(
		artifact,
		singletonVariables,
		protocolEnvelope(root),
		'network'
	);

	const snapshot = replica.read(artifact, expandedVariables);
	assert.equal(snapshot.status, 'ready');
	assert.equal(snapshot.complete, true);
	assert.equal(snapshot.stale, false);
	assert.deepEqual(snapshot.data, {
		[root.responseKey]: {
			id: '7',
			title: 'from the Rust artifact'
		}
	});
});
