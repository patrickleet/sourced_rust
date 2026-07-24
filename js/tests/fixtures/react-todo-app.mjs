import { createElement } from 'react';

import { useDistributedQuery } from '../../dist/react/index.js';
import {
	OpenTodosArtifact,
	TodoByIdArtifact
} from './adapter-conformance.mjs';

/**
 * Minimal generated-artifact consumer. It owns no cache, transport, auth, or
 * command reconciliation logic; the command prop has the same nested shape as
 * a generated ReplicaCommandRuntime.
 */
export function ReactTodoApp({ commands, selectedId = 'todo-1' }) {
	const todos = useDistributedQuery(OpenTodosArtifact);
	const todo = useDistributedQuery(TodoByIdArtifact, { id: selectedId });
	const rows = todos.data.todos ?? [];
	const selected = todo.data.todo;

	return createElement(
		'main',
		{
			'data-list-status': todos.status,
			'data-detail-status': todo.status
		},
		createElement(
			'ul',
			{ 'data-testid': 'todo-list' },
			...rows.map((row) =>
				createElement(
					'li',
					{ key: row.id, 'data-todo-id': row.id },
					createElement('span', { 'data-testid': `list-${row.id}` }, row.title),
					createElement(
						'button',
						{
							type: 'button',
							onClick: () => commands.todo.complete({ todoId: row.id })
						},
						'Complete'
					)
				)
			)
		),
		createElement(
			'aside',
			{
				'data-testid': 'todo-detail',
				'data-todo-id': selected?.id
			},
			selected?.title ?? 'Loading'
		)
	);
}
