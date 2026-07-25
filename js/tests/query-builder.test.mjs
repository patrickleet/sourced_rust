import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
	defineQuery,
	gqlEnum,
	gqlVar,
	lowerQuerySpecToGraphql,
	QUERY_SPEC_VERSION
} from '../dist/query/index.js';

test('defineQuery builds portable QuerySpec JSON', () => {
	const spec = defineQuery('Todos')
		.from('todos')
		.orderBy({ status: 'asc' }, { todo_id: 'asc' })
		.select({
			todo_id: true,
			owner_id: true,
			title: true,
			status: true
		})
		.load()
		.build();

	assert.equal(spec.version, QUERY_SPEC_VERSION);
	assert.equal(spec.name, 'Todos');
	assert.equal(spec.load, true);
	assert.equal(spec.live, undefined);
	assert.deepEqual(spec.roots[0].args.order_by, [
		{ status: { $enum: 'asc' } },
		{ todo_id: { $enum: 'asc' } }
	]);
});

test('toGraphql matches the Rust QuerySpec lowering shape', () => {
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

test('supports variables, where, live, and nested select', () => {
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
});

test('lowerQuerySpecToGraphql accepts hand-written IR', () => {
	const graphql = lowerQuerySpecToGraphql({
		version: 1,
		name: 'ById',
		roots: [
			{
				field: 'todos_by_pk',
				args: { todo_id: gqlVar('id') },
				select: { todo_id: true, title: true }
			}
		],
		variables: [{ name: 'id', type: 'ID!' }]
	});
	assert.match(graphql, /^query ById\(\$id: ID!\) \{/);
	assert.match(graphql, /todos_by_pk\(todo_id: \$id\)/);
});

test('rejects incomplete builders and invalid names', () => {
	assert.throws(() => defineQuery('Todos').build(), /from\(field\)/);
	assert.throws(
		() => defineQuery('Todos').from('todos').build(),
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
