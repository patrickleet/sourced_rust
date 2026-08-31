import assert from 'node:assert/strict';
import test from 'node:test';

import {
	distributedReloadLifecycle,
	registerDistributedReloadClient
} from '../dist/sveltekit/index.js';

test('prepared reload capsule survives the bounded activation transaction', async () => {
	const capsuleKey = '@hops-ops/distributed/reload-capsule/v1';
	const values = new Map();
	const deadlineUnixMs = Date.now() + 10_000;
	const generation = (generationId) => ({
		generationId,
		releaseId: `release-${generationId}`,
		topologyId: 'topology-1',
		compatibilityId: 'compatible-1'
	});
	globalThis.sessionStorage = {
		getItem: (key) => values.get(key) ?? null,
		setItem: (key, value) => values.set(key, String(value)),
		removeItem: (key) => values.delete(key)
	};
	globalThis.document = {
		querySelector: () => ({ getAttribute: () => 'generation-a' })
	};
	globalThis.window = {
		location: { href: 'http://localhost:5180/todos', reload() {} },
		dispatchEvent() {}
	};
	globalThis.CustomEvent = class {
		constructor(type, options) {
			this.type = type;
			this.detail = options?.detail;
		}
	};
	globalThis.fetch = async (_url, options) => {
		if (options?.method === 'POST') return { status: 204, ok: true };
		return {
			status: 200,
			ok: true,
			json: async () => ({
				schemaVersion: 1,
				phase: 'preparing',
				active: generation('generation-a'),
				pending: generation('generation-b'),
				transitionId: 'transition-prepare-1',
				deadlineUnixMs
			})
		};
	};
	const unregister = registerDistributedReloadClient(
		{ scope: undefined, dehydrate() { return {}; }, hydrate() { return false; } },
		undefined,
		{ key: 'public-surface' }
	);
	try {
		const timeout = Date.now() + 1_000;
		while (values.get(capsuleKey) === undefined) {
			if (Date.now() >= timeout) throw new Error('reload capsule was not prepared');
			await new Promise((resolve) => setTimeout(resolve, 10));
		}
		const capsule = JSON.parse(values.get(capsuleKey));
		assert.equal(capsule.phase, 'prepared');
		assert.ok(capsule.expiresAtUnixMs >= deadlineUnixMs + 120_000);
	} finally {
		unregister();
		distributedReloadLifecycle().destroy();
	}
});
