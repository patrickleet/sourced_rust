import assert from 'node:assert/strict';
import test from 'node:test';

import {
	validateDistributedReloadLocation,
	validateDistributedReloadState
} from '../dist/sveltekit/index.js';

test('reload state accepts finite declared JSON and rejects secret-like keys', () => {
	const state = { panel: 'todos', filters: ['open'], scroll: 42 };
	assert.equal(validateDistributedReloadState(state), state);
	for (const key of ['accessToken', 'password', 'authorization', 'session_cookie']) {
		assert.throws(() => validateDistributedReloadState({ [key]: 'must-not-cross' }));
	}
});

test('reload location preserves ordinary routing and rejects auth callback material', () => {
	assert.equal(
		validateDistributedReloadLocation(new URL('https://app.test/todos?status=open#mine')),
		'/todos?status=open#mine'
	);
	for (const query of ['code=oauth-code', 'access_token=jwt', 'session=value', 'state=csrf']) {
		assert.throws(() =>
			validateDistributedReloadLocation(new URL(`https://app.test/callback?${query}`))
		);
	}
	assert.throws(() =>
		validateDistributedReloadLocation(
			new URL('https://app.test/callback#access_token=jwt&token_type=bearer')
		)
	);
});

test('reload state rejects cycles, excessive depth, and values over one MiB', () => {
	const cyclic = {};
	cyclic.self = cyclic;
	assert.throws(() => validateDistributedReloadState(cyclic));
	let deep = 'leaf';
	for (let index = 0; index < 34; index += 1) deep = { next: deep };
	assert.throws(() => validateDistributedReloadState(deep));
	assert.throws(() => validateDistributedReloadState('x'.repeat(1024 * 1024)));
});
