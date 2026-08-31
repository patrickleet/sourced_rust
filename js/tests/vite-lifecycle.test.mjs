import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { distributedLifecycle } from '../dist/sveltekit/vite.js';

const hash = (byte) => `sha256:${byte.repeat(64)}`;

function invoke(handler, method, headers = {}, body = '') {
	return new Promise((resolve, reject) => {
		const request = new EventEmitter();
		request.method = method;
		request.headers = headers;
		const response = {
			statusCode: 200,
			headers: {},
			setHeader(name, value) {
				this.headers[name] = value;
			},
			end(value = '') {
				resolve({ status: this.statusCode, headers: this.headers, body: String(value) });
			}
		};
		try {
			handler(request, response);
			queueMicrotask(() => {
				if (body) request.emit('data', Buffer.from(body));
				request.emit('end');
			});
		} catch (error) {
			reject(error);
		}
	});
}

test('Vite lifecycle side channel registers browsers and writes bounded acknowledgements', async () => {
	const root = await mkdtemp(join(tmpdir(), 'distributed-vite-lifecycle-'));
	await mkdir(root, { recursive: true });
	const transitionId = hash('c');
	await writeFile(
		join(root, 'dev.json'),
		JSON.stringify({
			schemaVersion: 1,
			phase: 'preparing',
			active: {
				generationId: hash('a'),
				releaseId: hash('b'),
				topologyId: hash('e'),
				compatibilityId: hash('b')
			},
			pending: {
				generationId: transitionId,
				releaseId: hash('d'),
				topologyId: hash('e'),
				compatibilityId: hash('d')
			},
			transitionId,
			deadlineUnixMs: Date.now() + 3_000
		})
	);
	const previous = process.env.DISTRIBUTED_LIFECYCLE_DIR;
	process.env.DISTRIBUTED_LIFECYCLE_DIR = root;
	let handler;
	distributedLifecycle().configureServer({
		middlewares: {
			use(path, candidate) {
				assert.equal(path, '/__distributed/lifecycle');
				handler = candidate;
			}
		}
	});
	try {
		const participantId = 'browser_participant_1234';
		const state = await invoke(handler, 'GET', {
			'x-distributed-participant': participantId
		});
		assert.equal(state.status, 200);
		assert.equal(JSON.parse(state.body).transitionId, transitionId);
		assert.equal(
			JSON.parse(
				await readFile(
					join(root, 'dev-control', 'participants', `${participantId}.json`),
					'utf8'
				)
			).seenAtUnixMs > 0,
			true
		);

		const acknowledged = await invoke(
			handler,
			'POST',
			{ 'content-type': 'application/json' },
			JSON.stringify({ transitionId, participantId, ok: true })
		);
		assert.equal(acknowledged.status, 204);
		assert.deepEqual(
			JSON.parse(
				await readFile(
					join(root, 'dev-control', 'acks', transitionId, `${participantId}.json`),
					'utf8'
				)
			),
			{ ok: true }
		);
		const invalid = await invoke(
			handler,
			'POST',
			{ 'content-type': 'application/json' },
			JSON.stringify({ transitionId: '../../escape', participantId, ok: true })
		);
		assert.equal(invalid.status, 400);
	} finally {
		if (previous === undefined) delete process.env.DISTRIBUTED_LIFECYCLE_DIR;
		else process.env.DISTRIBUTED_LIFECYCLE_DIR = previous;
	}
});

test('lifecycle suppresses linked framework HMR until generation activation', async () => {
	const root = await mkdtemp(join(tmpdir(), 'distributed-vite-framework-'));
	const dist = join(root, 'node_modules/@hops-ops/distributed/dist');
	await mkdir(dist, { recursive: true });
	await mkdir(join(dist, 'sveltekit'), { recursive: true });
	await writeFile(join(dist, 'sveltekit/lifecycle.js'), 'export {};\n');
	const module = {};
	const invalidated = [];
	const previous = process.env.DISTRIBUTED_LIFECYCLE_DIR;
	process.env.DISTRIBUTED_LIFECYCLE_DIR = root;
	try {
		const plugin = distributedLifecycle();
		plugin.configResolved({ root });
		assert.deepEqual(
			plugin.handleHotUpdate({
				file: join(dist, 'sveltekit/lifecycle.js'),
				modules: [module],
				server: {
					moduleGraph: {
						invalidateModule(candidate) {
							invalidated.push(candidate);
						}
					}
				}
			}),
			[]
		);
		assert.deepEqual(invalidated, [module]);
		assert.equal(
			plugin.handleHotUpdate({
				file: join(root, 'src/app.ts'),
				server: { moduleGraph: { invalidateModule() {} } }
			}),
			undefined
		);
	} finally {
		if (previous === undefined) delete process.env.DISTRIBUTED_LIFECYCLE_DIR;
		else process.env.DISTRIBUTED_LIFECYCLE_DIR = previous;
	}
});
