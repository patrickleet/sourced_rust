import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import {
	defineQuery,
	gqlEnum,
	gqlVar,
	materializeClientDocuments
} from '../dist/query/index.js';

test('defineQuery materializes GraphQL document text', () => {
	const graphql = defineQuery('Todos')
		.from('todos')
		.orderBy({ status: 'asc' }, { todo_id: 'asc' })
		.select({
			todo_id: true,
			owner_id: true,
			title: true,
			status: true
		})
		.load()
		.toGraphql();

	assert.equal(
		graphql,
		'query Todos @load {\n  todos(order_by: [{status: asc}, {todo_id: asc}]) {\n    todo_id\n    owner_id\n    title\n    status\n  }\n}\n'
	);
});

test('supports variables, where, live, and nested select as GraphQL', () => {
	const graphql = defineQuery('ChatMessages')
		.from('chat_messages')
		.variable('roomId', 'String!')
		.where({ room_id: { _eq: gqlVar('roomId') } })
		.orderBy({ created_at: 'asc' })
		.select({
			message_id: true,
			body: true,
			author: { display_name: true }
		})
		.load()
		.live()
		.toGraphql();

	assert.equal(
		graphql,
		'query ChatMessages($roomId: String!) @load @live {\n  chat_messages(where: {room_id: {_eq: $roomId}}, order_by: [{created_at: asc}]) {\n    message_id\n    body\n    author {\n      display_name\n    }\n  }\n}\n'
	);
	assert.match(graphql, /^query /);
	assert.doesNotMatch(graphql, /"version"/);
});

test('rejects incomplete builders and invalid names', () => {
	assert.throws(() => defineQuery('Todos').toGraphql(), /from\(field\)/);
	assert.throws(
		() => defineQuery('Todos').from('todos').toGraphql(),
		/select\(/
	);
	assert.throws(() => defineQuery('bad-name'), /valid GraphQL name/);
	assert.throws(
		() => defineQuery('Todos').from('todos').orderBy({ status: 'sideways' }),
		/asc.*desc/
	);
	assert.throws(
		() =>
			defineQuery('Todos')
				.from('todos')
				.select({ title: false }),
		/omit excluded fields/
	);
	assert.throws(() => gqlEnum('not valid'), /valid GraphQL name/);
});

test('materializeClientDocuments evaluates .query.ts to GraphQL files', async () => {
	const root = await mkdtemp(join(tmpdir(), 'distributed-query-mat-'));
	const outDir = join(root, 'out');
	const routes = join(root, 'src', 'routes', 'todos');
	await mkdir(routes, { recursive: true });

	const queryPackage = join(process.cwd(), 'dist/query/index.js');
	const queryPath = join(routes, '+page.query.ts');
	await writeFile(
		queryPath,
		`
import { defineQuery } from ${JSON.stringify(queryPackage)};

export default defineQuery('Todos')
  .from('todos')
  .orderBy({ status: 'asc' })
  .select({ todo_id: true, title: true })
  .load();
`,
		'utf8'
	);

	await writeFile(
		join(routes, 'notes.graphql'),
		'query Notes { todos { todo_id } }\n',
		'utf8'
	);

	try {
		const materialized = await materializeClientDocuments({
			cwd: root,
			patterns: [
				'src/routes/todos/+page.query.ts',
				'src/routes/todos/notes.graphql'
			],
			outDir
		});

		assert.equal(materialized.documents.length, 2);
		const { readFile } = await import('node:fs/promises');
		const fromTs = await readFile(
			join(outDir, 'src/routes/todos/+page.query.ts'),
			'utf8'
		);
		assert.match(fromTs, /^query Todos @load \{/);
		assert.match(fromTs, /todo_id/);
		assert.doesNotMatch(fromTs, /defineQuery/);

		const fromGraphql = await readFile(
			join(outDir, 'src/routes/todos/notes.graphql'),
			'utf8'
		);
		assert.equal(fromGraphql, 'query Notes { todos { todo_id } }\n');
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});
