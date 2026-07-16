<script lang="ts">
	/**
	 * Admin: document store + force-archive command pipeline.
	 */
	import { onDestroy } from 'svelte';
	import { useGraphql, fx } from '$lib/gql';
	import { sessionDisplayName } from '$lib/session';
	import { adminTodos } from './admin.resource';
	import type { AdminTodoRow } from './admin.resource';

	let { data } = $props();

	let actionError = $state<string | null>(null);
	let busy = $state(false);

	const who = $derived(sessionDisplayName(data.session));
	const listLimit = $derived(data.listLimit ?? 100);

	const gql = useGraphql(() => data, {
		runEffects: (effects) => {
			for (const e of effects) {
				if (e.kind === 'alert') actionError = e.message;
			}
		}
	});

	const list = gql.store({
		document: adminTodos.query,
		initialData: { todos: data.todos ?? [] },
		select: (d: { todos?: AdminTodoRow[] }) => d?.todos ?? []
	});

	$effect(() => {
		list.seed({ todos: data.todos ?? [] });
	});

	onDestroy(() => list.destroy());

	const owners = $derived([...new Set($list.data.map((t) => t.owner_id))].sort());
	const open = $derived($list.data.filter((t) => t.status !== 'archived'));
	const atCap = $derived($list.data.length >= listLimit);

	async function forceArchive(todo_id: string) {
		if (busy) return;
		const target = $list.data.find((t) => t.todo_id === todo_id);
		if (!target || target.status === 'archived') return;

		actionError = null;
		busy = true;
		const result = await gql.commands.todosForceArchive(
			{ todo_id },
			{
				result: { kind: 'fact' },
				// Async projector — never refetch on the command path.
				reconcile: { kind: 'none' },
				optimistic: {
					targets: [list.target('todos', 'todo_id')],
					row: { ...target, status: 'archived' }
				},
				onError: ({ errors }) => [
					fx.alert(errors[0]?.message ?? 'force archive failed')
				]
			}
		);
		busy = false;

		if (result.errors?.length || !result.data) {
			if (!actionError) actionError = result.errors?.[0]?.message ?? 'force archive failed';
			return;
		}
		// Soft catch-up after projector lag (not on the command success path).
		window.setTimeout(() => {
			void list.refetch();
		}, 800);
	}
</script>

<section class="ad-page">
	<header class="ad-header">
		<div class="ad-kicker">
			<span class="ad-badge">admin</span>
			Role-scoped GraphQL
		</div>
		<h1 class="ad-title">All field notes</h1>
		<p class="ad-lede">
			Signed in as <strong>{who}</strong> with engine role
			<code>{data.engineRole}</code>. Query uses the same
			<code>todos</code> field without the owner filter.
			<strong>Force archive</strong> calls
			<code>todos_force_archive</code> — registered only for role
			<code>admin</code> (missing from the user GraphQL schema).
		</p>
	</header>

	{#if data.gqlError}
		<div class="ad-alert" role="alert">
			<strong>SSR GraphQL</strong>
			<span>{data.gqlError}</span>
		</div>
	{/if}
	{#if actionError}
		<div class="ad-alert" role="alert">
			<strong>Mutation</strong>
			<span>{actionError}</span>
		</div>
	{/if}

	<div class="ad-stats">
		<div class="ad-stat">
			<span class="ad-stat-n">{$list.data.length}</span>
			<span class="ad-stat-l">notes</span>
		</div>
		<div class="ad-stat">
			<span class="ad-stat-n">{owners.length}</span>
			<span class="ad-stat-l">owners</span>
		</div>
		<div class="ad-stat">
			<span class="ad-stat-n">{open.length}</span>
			<span class="ad-stat-l">active</span>
		</div>
	</div>

	{#if atCap}
		<p class="ad-cap" role="status">
			Showing first {listLimit} notes (bounded admin query). Refine filters or raise limit in
			<code>admin.gql</code> if needed.
		</p>
	{/if}

	{#if $list.data.length === 0}
		<p class="ad-empty">No notes in the read model yet. Create some as alice/bob on /todos.</p>
	{:else}
		<div class="ad-table-wrap">
			<table class="ad-table">
				<thead>
					<tr>
						<th>Owner</th>
						<th>Title</th>
						<th>Status</th>
						<th>Id</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each $list.data as t (t.todo_id)}
						<tr>
							<td class="ad-owner">{t.owner_id}</td>
							<td>{t.title}</td>
							<td><span class="ad-status" data-status={t.status}>{t.status}</span></td>
							<td class="ad-id">{t.todo_id}</td>
							<td class="ad-actions">
								{#if t.status !== 'archived'}
									<button
										type="button"
										class="ad-btn"
										disabled={busy}
										onclick={() => forceArchive(t.todo_id)}
									>
										Force archive
									</button>
								{:else}
									<span class="ad-muted">—</span>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<p class="ad-foot">
		<code>user</code> role cannot call <code>todos_force_archive</code> (field absent from user
		SDL). Suite T2c asserts that; admin mutation archives any owner's note.
	</p>
</section>

<style>
	.ad-page {
		--ink: var(--hops-navy, #1a2744);
		--ink-soft: rgba(26, 39, 68, 0.62);
		max-width: 56rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 4rem;
		font-family: var(--font-body, 'Lexend', system-ui, sans-serif);
		color: var(--ink);
	}

	.ad-header {
		margin-bottom: 1.5rem;
	}

	.ad-kicker {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--ink-soft);
		margin-bottom: 0.65rem;
	}

	.ad-badge {
		background: #1a2744;
		color: #f6e7c5;
		padding: 0.15rem 0.45rem;
		border-radius: 4px;
		letter-spacing: 0.08em;
	}

	.ad-title {
		font-size: clamp(1.75rem, 4vw, 2.35rem);
		font-weight: 800;
		letter-spacing: -0.03em;
		margin: 0 0 0.65rem;
	}

	.ad-lede {
		margin: 0;
		max-width: 42rem;
		line-height: 1.55;
		color: var(--ink-soft);
		font-size: 1.02rem;
	}

	.ad-lede code {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.88em;
		padding: 0.1em 0.3em;
		border-radius: 4px;
		background: rgba(26, 39, 68, 0.06);
	}

	.ad-alert {
		display: flex;
		gap: 0.75rem;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		border-radius: 12px;
		background: rgba(229, 62, 62, 0.08);
		border: 1px solid rgba(229, 62, 62, 0.25);
		color: #9b2c2c;
		font-size: 0.92rem;
	}

	.ad-stats {
		display: flex;
		gap: 0.75rem;
		margin-bottom: 1.25rem;
		flex-wrap: wrap;
	}

	.ad-stat {
		display: flex;
		align-items: baseline;
		gap: 0.4rem;
		padding: 0.45rem 0.85rem;
		border-radius: 999px;
		background: rgba(26, 39, 68, 0.04);
		border: 1px solid rgba(26, 39, 68, 0.1);
	}

	.ad-stat-n {
		font-weight: 800;
		font-variant-numeric: tabular-nums;
	}

	.ad-stat-l {
		font-size: 0.75rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--ink-soft);
	}

	.ad-empty {
		color: var(--ink-soft);
		font-style: italic;
	}

	.ad-cap {
		font-size: 0.88rem;
		color: var(--ink-soft);
		margin: 0 0 1rem;
		padding: 0.65rem 0.85rem;
		border-radius: 10px;
		background: rgba(230, 154, 45, 0.12);
		border: 1px solid rgba(230, 154, 45, 0.28);
	}

	.ad-cap code {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.9em;
	}

	.ad-table-wrap {
		overflow-x: auto;
		border-radius: 14px;
		border: 1px solid rgba(26, 39, 68, 0.1);
		background: #fbf8f1;
		box-shadow: 0 12px 40px rgba(15, 24, 41, 0.08);
	}

	.ad-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.92rem;
	}

	.ad-table th,
	.ad-table td {
		text-align: left;
		padding: 0.7rem 0.9rem;
		border-bottom: 1px solid rgba(26, 39, 68, 0.07);
		vertical-align: middle;
	}

	.ad-table th {
		font-size: 0.7rem;
		font-weight: 800;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--ink-soft);
		background: rgba(26, 39, 68, 0.03);
	}

	.ad-table tr:last-child td {
		border-bottom: none;
	}

	.ad-owner {
		font-weight: 700;
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.85em;
	}

	.ad-id {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.78em;
		color: var(--ink-soft);
		max-width: 7rem;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.ad-status {
		font-size: 0.72rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		padding: 0.15rem 0.45rem;
		border-radius: 999px;
		background: rgba(26, 39, 68, 0.08);
	}

	.ad-status[data-status='open'] {
		background: rgba(230, 154, 45, 0.18);
		color: #a66b12;
	}

	.ad-status[data-status='completed'] {
		background: rgba(56, 161, 105, 0.15);
		color: #276749;
	}

	.ad-status[data-status='archived'] {
		background: rgba(26, 39, 68, 0.08);
		color: var(--ink-soft);
	}

	.ad-actions {
		white-space: nowrap;
	}

	.ad-btn {
		font: inherit;
		font-size: 0.78rem;
		font-weight: 700;
		border: none;
		border-radius: 8px;
		padding: 0.4rem 0.7rem;
		cursor: pointer;
		background: rgba(229, 62, 62, 0.12);
		color: #9b2c2c;
	}

	.ad-btn:hover:not(:disabled) {
		background: rgba(229, 62, 62, 0.22);
	}

	.ad-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.ad-muted {
		color: var(--ink-soft);
		font-size: 0.85rem;
	}

	.ad-foot {
		margin-top: 1.5rem;
		font-size: 0.88rem;
		line-height: 1.5;
		color: var(--ink-soft);
	}

	.ad-foot code {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.9em;
	}
</style>
