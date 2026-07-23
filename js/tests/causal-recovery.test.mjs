import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
	CausalReceiptError,
	bindCommands,
	defineCausalProtocol,
	defineCommand,
	defineCommands,
	executeCommand
} from '@hops-ops/distributed/commands';
import { createGraphqlClient } from '@hops-ops/distributed';
import { QueryCache, cacheKey } from '@hops-ops/distributed/cache';

const COMMAND_DOCUMENT =
	'mutation Client_todos_create($commandId: ID!, $input: TodoInput!) { todos_create(commandId: $commandId, input: $input) { id title } }';
const STATUS_DOCUMENT =
	'query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }';
const TODOS_DOCUMENT = 'query Todos { todos { id title } }';
const SCHEMA_HASH = `sha256:${'a'.repeat(64)}`;
const COMMAND_HASH = `sha256:${'b'.repeat(64)}`;
const STATUS_HASH = `sha256:${'c'.repeat(64)}`;
const COMMAND_ID = '01890f47-0f00-7000-8000-000000000001';
const OTHER_COMMAND_ID = '01890f47-0f00-7000-8000-000000000002';

const PROTOCOL = defineCausalProtocol({
	protocolVersion: 2,
	schemaHash: SCHEMA_HASH,
	commandStatus: {
		name: 'Distributed_CommandStatus',
		document: STATUS_DOCUMENT,
		operationHash: STATUS_HASH
	}
});

test('lost mutation response retries the exact command ID and returns the original payload API', async () => {
	const calls = [];
	const committed = new Set();
	let durableWrites = 0;
	const client = {
		async request(document, variables) {
			assert.equal(document, COMMAND_DOCUMENT);
			calls.push(structuredClone(variables));
			if (!committed.has(variables.commandId)) {
				committed.add(variables.commandId);
				durableWrites += 1;
			}
			if (calls.length === 1) throw new Error('response lost after commit');
			return mutationResult(variables.commandId, 'accepted_pending_projection');
		}
	};

	const result = await executeCommand(
		client,
		causalCommand(),
		{ title: 'same payload' },
		{
			commandId: COMMAND_ID,
			recovery: { transportRetries: 1, initialDelayMs: 0, maxDelayMs: 1 }
		}
	);

	assert.deepEqual(result.data, { id: 'one', title: 'same payload' });
	assert.equal(result.receipt?.commandId, COMMAND_ID);
	assert.equal(result.receipt?.state, 'accepted_pending_projection');
	assert.equal(durableWrites, 1);
	assert.equal(calls.length, 2);
	assert.deepEqual(calls[0], calls[1]);
	assert.deepEqual(calls[0], {
		commandId: COMMAND_ID,
		input: { title: 'same payload' }
	});
});

test('same-ID retries use a frozen wire snapshot after caller input mutation', async () => {
	const firstAttempt = deferred();
	const firstStarted = deferred();
	const calls = [];
	const input = { title: 'original', nested: { priority: 1 } };
	let attempts = 0;
	const execution = executeCommand(
		{
			async request(_document, variables) {
				attempts += 1;
				calls.push(structuredClone(variables));
				if (attempts === 1) {
					firstStarted.resolve();
					await firstAttempt.promise;
					throw new Error('response lost after commit');
				}
				return mutationResult(
					variables.commandId,
					'accepted_pending_projection'
				);
			}
		},
		causalCommand(),
		input,
		{
			commandId: COMMAND_ID,
			recovery: {
				transportRetries: 1,
				initialDelayMs: 0,
				maxDelayMs: 1
			}
		}
	);
	await firstStarted.promise;
	input.title = 'mutated';
	input.nested.priority = 99;
	firstAttempt.resolve();
	await execution;

	assert.equal(calls.length, 2);
	assert.deepEqual(calls[0], calls[1]);
	assert.deepEqual(calls[0].input, {
		title: 'original',
		nested: { priority: 1 }
	});
});

test('duplicate consumers and tabs safely replay one server identity and one receipt coalesces status', async () => {
	const committed = new Set();
	let durableWrites = 0;
	let statusCalls = 0;
	const statusOptions = [];
	const client = {
		async request(document, variables, options) {
			if (document === STATUS_DOCUMENT) {
				statusCalls += 1;
				statusOptions.push(options);
				await Promise.resolve();
				return statusResult(variables.commandId, 'projected', {
					command: commandMetadata(variables.commandId, 'projected')
				});
			}
			if (!committed.has(variables.commandId)) {
				committed.add(variables.commandId);
				durableWrites += 1;
			}
			return mutationResult(variables.commandId, 'accepted_pending_projection');
		}
	};
	const options = {
		commandId: COMMAND_ID,
		recovery: { initialDelayMs: 0, maxDelayMs: 1 }
	};

	const [first, second] = await Promise.all([
		executeCommand(client, causalCommand(), { title: 'same' }, options),
		executeCommand(client, causalCommand(), { title: 'same' }, options)
	]);
	assert.equal(durableWrites, 1);
	assert.equal(first.receipt?.commandId, second.receipt?.commandId);

	const projected = first.receipt.projected;
	assert.equal(projected, first.receipt.projected);
	const [left, right] = await Promise.all([projected, projected]);
	assert.equal(left.state, 'projected');
	assert.equal(right.state, 'projected');
	assert.equal(statusCalls, 1);
	assert.deepEqual(statusOptions, [{ cache: 'skip' }]);
});

test('framework status polling bypasses the application query cache', async () => {
	const cache = new QueryCache();
	const client = createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth: () => ({ accessToken: 'opaque-token' }),
		cache,
		fetch: async (_input, init) => {
			const request = JSON.parse(String(init.body));
			const commandId = request.variables.commandId;
			const body =
				request.query === STATUS_DOCUMENT
					? statusResult(commandId, 'projected', {
							command: commandMetadata(commandId, 'projected')
						})
					: mutationResult(commandId, 'accepted_pending_projection');
			const { status: _status, ...graphqlBody } = body;
			return new Response(JSON.stringify(graphqlBody), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			});
		}
	});
	const result = await executeCommand(
		client,
		causalCommand(),
		{ title: 'no status cache row' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(1) }
	);
	assert.equal((await result.receipt.projected).state, 'projected');
	assert.equal(
		cache.get(cacheKey(STATUS_DOCUMENT, { commandId: COMMAND_ID })),
		undefined
	);
});

test('unknown and provisional in-progress states poll until a full projected receipt materializes', async () => {
	const states = [
		statusResult(COMMAND_ID, 'unknown'),
		statusResult(COMMAND_ID, 'in_progress', {
			command: commandMetadata(COMMAND_ID, 'in_progress', { expects: [] })
		}),
		statusResult(COMMAND_ID, 'projected', {
			command: commandMetadata(COMMAND_ID, 'projected')
		})
	];
	const seen = [];
	const client = {
		async request(document, variables, options) {
			if (document === COMMAND_DOCUMENT) {
				return mutationResult(variables.commandId, 'in_progress', {
					expects: []
				});
			}
			seen.push({ document, variables: structuredClone(variables), options });
			return states.shift();
		}
	};
	const result = await executeCommand(
		client,
		causalCommand(),
		{ title: 'eventual' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(4) }
	);

	const projected = await result.receipt.projected;
	assert.equal(projected.state, 'projected');
	assert.equal(result.receipt.state, 'projected');
	assert.equal(seen.length, 3);
	assert.ok(seen.every((call) => call.document === STATUS_DOCUMENT));
	assert.ok(
		seen.every(
			(call) =>
				call.variables.commandId === COMMAND_ID &&
				call.options.cache === 'skip'
		)
	);
});

test('status correlation rejects scope, schema, identity, causation, consistency, and expectation drift', async (t) => {
	const cases = [
		{
			name: 'scope',
			result: statusResult(COMMAND_ID, 'projected', {
				cacheScope: 'scope-b',
				command: commandMetadata(COMMAND_ID, 'projected')
			}),
			code: 'CAUSAL_SCOPE_INVALIDATED'
		},
		{
			name: 'schema',
			result: statusResult(COMMAND_ID, 'projected', {
				schemaHash: `sha256:${'d'.repeat(64)}`,
				command: commandMetadata(COMMAND_ID, 'projected')
			}),
			code: 'CAUSAL_SCHEMA_INVALIDATED'
		},
		{
			name: 'command identity',
			result: statusResult(COMMAND_ID, 'projected', {
				command: commandMetadata(OTHER_COMMAND_ID, 'projected')
			}),
			code: 'CAUSAL_PROTOCOL_INVALID'
		},
		{
			name: 'operation identity',
			result: statusResult(COMMAND_ID, 'projected', {
				operation: `sha256:${'e'.repeat(64)}`,
				command: commandMetadata(COMMAND_ID, 'projected')
			}),
			code: 'CAUSAL_PROTOCOL_INVALID'
		},
		{
			name: 'causation',
			result: statusResult(COMMAND_ID, 'projected', {
				command: commandMetadata(COMMAND_ID, 'projected', {
					causationId: 'cause-two'
				})
			}),
			code: 'CAUSAL_PROTOCOL_INVALID'
		},
		{
			name: 'consistency',
			result: statusResult(COMMAND_ID, 'projected', {
				command: commandMetadata(COMMAND_ID, 'projected', {
					consistency: 'accepted'
				})
			}),
			code: 'CAUSAL_PROTOCOL_INVALID'
		},
		{
			name: 'expectation token',
			result: statusResult(COMMAND_ID, 'projected', {
				command: commandMetadata(COMMAND_ID, 'projected', {
					expects: [
						{
							projection: 'project-todos',
							model: 'Todo',
							scopeToken: 'drifted-projection-scope'
						}
					]
				})
			}),
			code: 'CAUSAL_PROTOCOL_INVALID'
		}
	];

	for (const item of cases) {
		await t.test(item.name, async () => {
			const client = scriptedClient([item.result]);
			const command = await executeCommand(
				client,
				causalCommand(),
				{ title: item.name },
				{ commandId: COMMAND_ID, recovery: fastRecovery(1) }
			);
			await assert.rejects(command.receipt.projected, (error) => {
				assert.ok(error instanceof CausalReceiptError);
				assert.equal(error.code, item.code);
				return true;
			});
		});
	}
});

test('status data and receipt state must agree exactly', async () => {
	const client = scriptedClient([
		statusResult(COMMAND_ID, 'projected', {
			command: commandMetadata(COMMAND_ID, 'projection_failed')
		})
	]);
	const result = await executeCommand(
		client,
		causalCommand(),
		{ title: 'state drift' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(1) }
	);
	await assert.rejects(result.receipt.projected, (error) => {
		assert.equal(error.code, 'CAUSAL_PROTOCOL_INVALID');
		return true;
	});
});

test('projected rejects every authoritative terminal state with a typed safe error', async (t) => {
	const terminals = [
		['rejected', 'CAUSAL_COMMAND_REJECTED'],
		['projection_failed', 'CAUSAL_PROJECTION_FAILED'],
		['expired', 'CAUSAL_COMMAND_EXPIRED']
	];
	for (const [state, code] of terminals) {
		await t.test(state, async () => {
			const terminal = statusResult(COMMAND_ID, state, {
				...(state === 'expired'
					? {}
					: { command: commandMetadata(COMMAND_ID, state) })
			});
			const client = scriptedClient([terminal]);
			const result = await executeCommand(
				client,
				causalCommand(),
				{ title: state },
				{ commandId: COMMAND_ID, recovery: fastRecovery(1) }
			);
			await assert.rejects(result.receipt.projected, (error) => {
				assert.ok(error instanceof CausalReceiptError);
				assert.equal(error.code, code);
				assert.equal(error.state, state);
				return true;
			});
		});
	}
});

test('polling is bounded by attempts/deadline and abort remains a caller-local outcome', async () => {
	let statusCalls = 0;
	const client = {
		async request(document, variables) {
			if (document === COMMAND_DOCUMENT) {
				return mutationResult(variables.commandId, 'in_progress', { expects: [] });
			}
			statusCalls += 1;
			return statusResult(variables.commandId, 'unknown');
		}
	};
	const result = await executeCommand(
		client,
		causalCommand(),
		{ title: 'bounded' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(2) }
	);
	await assert.rejects(result.receipt.projected, (error) => {
		assert.equal(error.code, 'CAUSAL_DEADLINE_EXCEEDED');
		assert.equal(error.state, 'unknown');
		return true;
	});
	assert.equal(statusCalls, 2);

	const controller = new AbortController();
	controller.abort();
	await assert.rejects(
		result.receipt.waitForProjected({ signal: controller.signal }),
		(error) => {
			assert.equal(error.code, 'CAUSAL_ABORTED');
			return true;
		}
	);
	assert.equal(statusCalls, 2);
});

test('a deadline bounds a hung status request and aborting one waiter does not cancel another', async () => {
	const never = new Promise(() => {});
	let hungStatusCalls = 0;
	const hung = await executeCommand(
		{
			async request(document, variables) {
				if (document === COMMAND_DOCUMENT) {
					return mutationResult(variables.commandId, 'in_progress', {
						expects: []
					});
				}
				hungStatusCalls += 1;
				return hungStatusCalls === 1
					? never
					: statusResult(variables.commandId, 'projected', {
							command: commandMetadata(variables.commandId, 'projected')
						});
			}
		},
		causalCommand(),
		{ title: 'hung' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(2) }
	);
	await assert.rejects(
		hung.receipt.waitForProjected({
			deadlineMs: 5,
			initialDelayMs: 0,
			maxDelayMs: 1
		}),
		(error) => {
			assert.equal(error.code, 'CAUSAL_DEADLINE_EXCEEDED');
			return true;
		}
	);
	assert.equal(
		(
			await hung.receipt.waitForProjected({
				deadlineMs: 100,
				initialDelayMs: 0,
				maxDelayMs: 1
			})
		).state,
		'projected'
	);
	assert.equal(hungStatusCalls, 2);

	const status = deferred();
	let statusCalls = 0;
	const shared = await executeCommand(
		{
			async request(document, variables) {
				if (document === COMMAND_DOCUMENT) {
					return mutationResult(variables.commandId, 'in_progress', {
						expects: []
					});
				}
				statusCalls += 1;
				return status.promise;
			}
		},
		causalCommand(),
		{ title: 'shared' },
		{ commandId: OTHER_COMMAND_ID, recovery: fastRecovery(2) }
	);
	const controller = new AbortController();
	const stopped = shared.receipt.waitForProjected({
		signal: controller.signal,
		deadlineMs: 1_000
	});
	const continuing = shared.receipt.waitForProjected({ deadlineMs: 1_000 });
	await Promise.resolve();
	controller.abort();
	await assert.rejects(stopped, (error) => {
		assert.equal(error.code, 'CAUSAL_ABORTED');
		return true;
	});
	status.resolve(
		statusResult(OTHER_COMMAND_ID, 'projected', {
			command: commandMetadata(OTHER_COMMAND_ID, 'projected')
		})
	);
	assert.equal((await continuing).state, 'projected');
	assert.equal(statusCalls, 1);
});

test('status auth and malformed status responses fail closed instead of being retried', async (t) => {
	await t.test('authorization', async () => {
		let statusCalls = 0;
		const client = scriptedClient([
			{
				errors: [{ message: 'hidden', extensions: { code: 'UNAUTHORIZED' } }],
				extensions: envelope(STATUS_HASH),
				status: 200
			}
		], () => {
			statusCalls += 1;
		});
		const result = await executeCommand(
			client,
			causalCommand(),
			{ title: 'auth' },
			{ commandId: COMMAND_ID, recovery: fastRecovery(8) }
		);
		await assert.rejects(result.receipt.projected, (error) => {
			assert.equal(error.code, 'CAUSAL_SCOPE_INVALIDATED');
			return true;
		});
		assert.equal(statusCalls, 1);
	});

	await t.test('malformed state', async () => {
		const client = scriptedClient([
			{
				data: { commandStatus: { state: 'surprise' } },
				extensions: envelope(STATUS_HASH),
				status: 200
			}
		]);
		const result = await executeCommand(
			client,
			causalCommand(),
			{ title: 'bad state' },
			{ commandId: COMMAND_ID, recovery: fastRecovery(8) }
		);
		await assert.rejects(result.receipt.projected, (error) => {
			assert.equal(error.code, 'CAUSAL_PROTOCOL_INVALID');
			return true;
		});
	});
});

test('an ambiguous status transport is retried within the same explicit bound', async () => {
	let statusCalls = 0;
	const client = {
		async request(document, variables) {
			if (document === COMMAND_DOCUMENT) {
				return mutationResult(variables.commandId, 'in_progress', { expects: [] });
			}
			statusCalls += 1;
			if (statusCalls === 1) throw new Error('status response lost');
			return statusResult(variables.commandId, 'projected', {
				command: commandMetadata(variables.commandId, 'projected')
			});
		}
	};
	const result = await executeCommand(
		client,
		causalCommand(),
		{ title: 'retry status' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(2) }
	);
	assert.equal((await result.receipt.projected).state, 'projected');
	assert.equal(statusCalls, 2);
});

test('TIMEOUT status is retryable while NOT_FOUND never fabricates expired', async () => {
	let timeoutCalls = 0;
	const retrying = await executeCommand(
		{
			async request(document, variables) {
				if (document === COMMAND_DOCUMENT) {
					return mutationResult(
						variables.commandId,
						'accepted_pending_projection'
					);
				}
				timeoutCalls += 1;
				if (timeoutCalls === 1) {
					return {
						errors: [
							{
								message: 'status timed out',
								extensions: { code: 'TIMEOUT' }
							}
						],
						status: 200
					};
				}
				if (timeoutCalls === 2) {
					return { status: 502 };
				}
				return statusResult(variables.commandId, 'projected', {
					command: commandMetadata(variables.commandId, 'projected')
				});
			}
		},
		causalCommand(),
		{ title: 'timeout then projected' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(3) }
	);
	assert.equal((await retrying.receipt.projected).state, 'projected');
	assert.equal(timeoutCalls, 3);

	const missing = await executeCommand(
		{
			async request(document, variables) {
				if (document === COMMAND_DOCUMENT) {
					return mutationResult(
						variables.commandId,
						'accepted_pending_projection'
					);
				}
				return {
					errors: [
						{
							message: 'wrong endpoint or unknown identity',
							extensions: { code: 'NOT_FOUND' }
						}
					],
					status: 404
				};
			}
		},
		causalCommand(),
		{ title: 'not missing by inference' },
		{ commandId: OTHER_COMMAND_ID, recovery: fastRecovery(2) }
	);
	await assert.rejects(missing.receipt.projected, (error) => {
		assert.equal(error.code, 'CAUSAL_PROTOCOL_INVALID');
		assert.notEqual(error.code, 'CAUSAL_COMMAND_EXPIRED');
		return true;
	});
	assert.notEqual(missing.receipt.state, 'expired');
});

test('late status attempts and sequential regressions cannot overwrite terminal state', async () => {
	const oldStatus = deferred();
	let statusCalls = 0;
	const result = await executeCommand(
		{
			async request(document, variables) {
				if (document === COMMAND_DOCUMENT) {
					return mutationResult(
						variables.commandId,
						'accepted_pending_projection'
					);
				}
				statusCalls += 1;
				if (statusCalls === 1) return oldStatus.promise;
				if (statusCalls === 2) {
					return statusResult(variables.commandId, 'projected', {
						command: commandMetadata(variables.commandId, 'projected')
					});
				}
				return statusResult(variables.commandId, 'unknown');
			}
		},
		causalCommand(),
		{ title: 'status fence' },
		{ commandId: COMMAND_ID, recovery: fastRecovery(3) }
	);
	await assert.rejects(
		result.receipt.waitForProjected({
			deadlineMs: 5,
			initialDelayMs: 0,
			maxDelayMs: 1
		}),
		(error) => {
			assert.equal(error.code, 'CAUSAL_DEADLINE_EXCEEDED');
			return true;
		}
	);
	assert.equal((await result.receipt.status()).state, 'projected');
	oldStatus.resolve(statusResult(COMMAND_ID, 'unknown'));
	await new Promise((resolve) => setImmediate(resolve));
	assert.equal(result.receipt.state, 'projected');
	assert.equal((await result.receipt.status()).state, 'projected');
	assert.equal(result.receipt.state, 'projected');
});

test('ambiguous HTTP and GraphQL failures retry, but correlated error receipts are observed', async (t) => {
	for (const failure of [
		{
			name: 'HTTP 502',
			result: {
				errors: [{ message: 'gateway lost the response' }],
				status: 502
			}
		},
		{
			name: 'HTTP 200 INTERNAL',
			result: {
				errors: [
					{
						message: 'resolver response was lost',
						extensions: { code: 'INTERNAL' }
					}
				],
				status: 200
			}
		},
		{
			name: 'untyped NOT_FOUND',
			result: {
				errors: [
					{
						message: 'unknown endpoint or command identity',
						extensions: { code: 'NOT_FOUND' }
					}
				],
				status: 404
			}
		}
	]) {
		await t.test(failure.name, async () => {
			let calls = 0;
			const result = await executeCommand(
				{
					async request(_document, variables) {
						calls += 1;
						return calls === 1
							? failure.result
							: mutationResult(
									variables.commandId,
									'accepted_pending_projection'
								);
					}
				},
				causalCommand(),
				{ title: failure.name },
				{
					commandId: COMMAND_ID,
					recovery: {
						transportRetries: 1,
						initialDelayMs: 0,
						maxDelayMs: 1
					}
				}
			);
			assert.equal(calls, 2);
			assert.equal(result.receipt.state, 'accepted_pending_projection');
		});
	}

	let correlatedCalls = 0;
	const correlated = await executeCommand(
		{
			async request(_document, variables) {
				correlatedCalls += 1;
				return {
					...mutationResult(
						variables.commandId,
						'accepted_pending_projection'
					),
					errors: [
						{
							message: 'partial resolver warning',
							extensions: { code: 'INTERNAL' }
						}
					],
					status: 500
				};
			}
		},
		causalCommand(),
		{ title: 'correlated' },
		{ commandId: OTHER_COMMAND_ID, recovery: { transportRetries: 8 } }
	);
	assert.equal(correlatedCalls, 1);
	assert.equal(correlated.receipt.state, 'accepted_pending_projection');

	let terminalCalls = 0;
	const terminal = await executeCommand(
		{
			async request(_document, variables) {
				terminalCalls += 1;
				return {
					...mutationResult(variables.commandId, 'expired'),
					errors: [
						{
							message: 'typed receipt expired',
							extensions: { code: 'NOT_FOUND' }
						}
					],
					status: 404
				};
			}
		},
		causalCommand(),
		{ title: 'typed expiry' },
		{ commandId: COMMAND_ID, recovery: { transportRetries: 8 } }
	);
	assert.equal(terminalCalls, 1);
	assert.equal(terminal.receipt.state, 'expired');
});

test('authoritative mutation errors are not retried and nominal success requires a receipt', async (t) => {
	for (const code of [
		'REJECTED',
		'COMMAND_ID_REUSE',
		'UNAUTHORIZED',
		'BAD_REQUEST'
	]) {
		await t.test(code, async () => {
			let calls = 0;
			const rejected = await executeCommand(
				{
					async request() {
						calls += 1;
						return {
							errors: [{ message: 'safe failure', extensions: { code } }],
							status: 200
						};
					}
				},
				causalCommand(),
				{ title: code },
				{ commandId: COMMAND_ID, recovery: { transportRetries: 8 } }
			);
			assert.equal(calls, 1);
			assert.equal(rejected.errors[0].extensions.code, code);
		});
	}

	let malformedCalls = 0;
	const malformed = await executeCommand(
		{
			async request() {
				malformedCalls += 1;
				return {
					data: { todos_create: { id: 'one' } },
					status: 200
				};
			}
		},
		causalCommand(),
		{ title: 'missing receipt' },
		{ commandId: COMMAND_ID, recovery: { transportRetries: 8 } }
	);
	assert.equal(malformedCalls, 1);
	assert.equal(
		malformed.errors[0].extensions.code,
		'CAUSAL_PROTOCOL_INVALID'
	);
	assert.notEqual(
		malformed.errors[0].extensions.code,
		'CAUSAL_TRANSPORT_AMBIGUOUS'
	);

	const transportParsed = await executeCommand(
		{
			async request() {
				return {
					errors: [
						{
							message: 'invalid protocol',
							extensions: { code: 'DISTRIBUTED_PROTOCOL_INVALID' }
						}
					],
					status: 200
				};
			}
		},
		causalCommand(),
		{ title: 'transport parser failure' },
		{ commandId: OTHER_COMMAND_ID, recovery: { transportRetries: 8 } }
	);
	assert.equal(
		transportParsed.errors[0].extensions.code,
		'CAUSAL_PROTOCOL_INVALID'
	);
});

test('accepted without finite projection work has no projected awaitable; accepted with one is invalid', async () => {
	const acceptedClient = {
		async request(_document, variables) {
			return mutationResult(variables.commandId, 'accepted', { expects: [] });
		}
	};
	const completed = await executeCommand(
		acceptedClient,
		causalCommand({ projects: false }),
		{ title: 'fire and done' },
		{ commandId: COMMAND_ID }
	);
	assert.equal(completed.receipt.state, 'accepted');
	assert.equal(completed.receipt.projected, undefined);

	const invalid = await executeCommand(
		acceptedClient,
		causalCommand({ projects: true }),
		{ title: 'missing pending state' },
		{ commandId: OTHER_COMMAND_ID }
	);
	assert.equal(invalid.errors[0].extensions.code, 'CAUSAL_PROTOCOL_INVALID');
	assert.equal(invalid.receipt.state, 'accepted');
});

test('a correlated pending receipt retains bound optimism while surfacing GraphQL errors', async () => {
	const cache = new QueryCache();
	const key = cacheKey(TODOS_DOCUMENT);
	cache.set(key, { data: { todos: [] }, updatedAt: 1 });
	const commands = bindCommands(
		{
			cache,
			async request(_document, variables) {
				return {
					...mutationResult(
						variables.commandId,
						'accepted_pending_projection'
					),
					errors: [
						{
							message: 'partial command resolver failure',
							extensions: { code: 'INTERNAL' }
						}
					],
					status: 500
				};
			}
		},
		defineCommands({ create: causalCommand() })
	);
	const result = await commands.create(
		{ id: 'optimistic', title: 'retained' },
		{
			commandId: COMMAND_ID,
			browser: true,
			optimistic: {
				targets: [
					{ document: TODOS_DOCUMENT, at: 'todos', by: 'id' }
				],
				row: { id: 'optimistic', title: 'retained' }
			}
		}
	);

	assert.equal(result.errors[0].extensions.code, 'INTERNAL');
	assert.equal(result.receipt.state, 'accepted_pending_projection');
	assert.deepEqual(cache.get(key).data.todos, [
		{ id: 'optimistic', title: 'retained' }
	]);
	assert.equal(cache.get(key).pending, true);
	assert.equal(cache.get(key).optimistic, true);
});

test('ambiguous causal pipeline keeps optimism while raw commands preserve legacy rollback behavior', async () => {
	const cache = new QueryCache();
	const key = cacheKey(TODOS_DOCUMENT);
	cache.set(key, { data: { todos: [] }, updatedAt: 1 });
	const commands = bindCommands(
		{
			cache,
			async request() {
				throw new Error('connection vanished');
			}
		},
		defineCommands({ create: causalCommand() })
	);
	const result = await commands.create(
		{ id: 'optimistic', title: 'kept' },
		{
			commandId: COMMAND_ID,
			recovery: { transportRetries: 0 },
			browser: true,
			optimistic: {
				targets: [{ document: TODOS_DOCUMENT, at: 'todos', by: 'id' }],
				row: { id: 'optimistic', title: 'kept' }
			}
		}
	);
	assert.equal(
		result.errors[0].extensions.code,
		'CAUSAL_TRANSPORT_AMBIGUOUS'
	);
	assert.deepEqual(cache.get(key).data.todos, [
		{ id: 'optimistic', title: 'kept' }
	]);

	const seen = [];
	const raw = defineCommand({
		field: 'todos_create',
		document: COMMAND_DOCUMENT,
		hasInput: true
	});
	await assert.rejects(
		executeCommand(
			{
				async request(_document, variables) {
					seen.push(variables);
					throw new Error('raw failure');
				}
			},
			raw,
			{ title: 'legacy' },
			{ commandId: COMMAND_ID, recovery: { transportRetries: 8 } }
		),
		/raw failure/
	);
	assert.deepEqual(seen, [{ input: { title: 'legacy' } }]);
});

function causalCommand({ projects = true } = {}) {
	return defineCommand({
		field: 'todos_create',
		document: COMMAND_DOCUMENT,
		hasInput: true,
		causal: {
			protocol: PROTOCOL,
			operationHash: COMMAND_HASH,
			projects
		}
	});
}

function scriptedClient(statuses, onStatus = () => {}) {
	return {
		async request(document, variables) {
			if (document === COMMAND_DOCUMENT) {
				return mutationResult(variables.commandId, 'accepted_pending_projection');
			}
			onStatus();
			const next = statuses.shift();
			assert.ok(next, 'status script exhausted');
			return next;
		}
	};
}

function mutationResult(commandId, state, overrides = {}) {
	return {
		data: {
			todos_create: {
				id: 'one',
				title: 'same payload'
			}
		},
		extensions: envelope(COMMAND_HASH, {
			command: commandMetadata(commandId, state, overrides)
		}),
		status: 200
	};
}

function statusResult(commandId, state, overrides = {}) {
	const { command, ...envelopeOverrides } = overrides;
	return {
		data: { commandStatus: { state } },
		extensions: envelope(STATUS_HASH, {
			...envelopeOverrides,
			...(command ? { command } : {})
		}),
		status: 200
	};
}

function envelope(operation, overrides = {}) {
	return {
		distributed: {
			protocolVersion: 2,
			schemaHash: SCHEMA_HASH,
			cacheScope: 'scope-a',
			operation,
			...overrides
		}
	};
}

function commandMetadata(commandId, state, overrides = {}) {
	return {
		commandId,
		causationId: 'cause-one',
		state,
		consistency: 'fact',
		expects: [
			{
				projection: 'project-todos',
				model: 'Todo',
				scopeToken: 'projection-scope'
			}
		],
		...overrides
	};
}

function fastRecovery(maxStatusAttempts) {
	return {
		maxStatusAttempts,
		deadlineMs: 1_000,
		initialDelayMs: 0,
		maxDelayMs: 1,
		backoffFactor: 1
	};
}

function deferred() {
	let resolve;
	const promise = new Promise((done) => {
		resolve = done;
	});
	return { promise, resolve };
}
