<script lang="ts">
	/**
	 * Admin: separate elevated generated surface + causal command runtime.
	 */
	import { AdminAllTodos, useCommands } from '$distributed/admin';
	import { sessionDisplayName } from '$lib/session';

	let { data } = $props();
	let actionError = $state<string | null>(null);
	let busy = $state(false);

	const who = $derived(sessionDisplayName(data.session));
	const listLimit = 100;

	const list = AdminAllTodos.use();
	const commands = useCommands();

	// The generated query/index plan owns collection order.
	const rows = $derived($list.complete ? $list.data.todos : []);
	const owners = $derived([...new Set(rows.map((todo) => todo.owner_id))].sort());
	const open = $derived(rows.filter((todo) => todo.status !== 'archived'));
	const atCap = $derived(rows.length >= listLimit);

	async function forceArchive(todo_id: string) {
		if (busy) return;
		const target = rows.find((todo) => todo.todo_id === todo_id);
		if (!target || target.status === 'archived') return;

		actionError = null;
		busy = true;
		try {
			await commands.todo.force_archive({ todo_id });
		} catch (error) {
			actionError = error instanceof Error ? error.message : 'force archive failed';
		} finally {
			busy = false;
		}
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
			<code>{data.engineRole}</code>. This nested layout installs a separate
			<code>fieldnote-admin</code> client, so elevated query and command artifacts
			cannot leak into the normal application bundle.
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
			<span class="ad-stat-n">{rows.length}</span>
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
			<code>+page.graphql</code> if needed.
		</p>
	{/if}

	{#if rows.length === 0}
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
					{#each rows as t (t.todo_id)}
						<tr data-todo-id={t.todo_id}>
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
		The normal <code>fieldnote</code> surface cannot even name
		<code>todo.force_archive</code>. The elevated artifact is generated only for this
		admin-gated component tree.
	</p>
</section>

<style>
	.ad-page {
		--ink: var(--wf-ink, #1c1c1a);
		--ink-soft: var(--wf-ink-soft, #5c5c56);
		--edge: var(--wf-line, #e2e0d9);
		--surface: var(--wf-bg-elevated, #fff);
		max-width: 56rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 4rem;
		font-family: var(--wf-sans, system-ui, sans-serif);
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
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--ink-soft);
		margin-bottom: 0.65rem;

	}

	.ad-badge {
		background: var(--ink);
		color: var(--wf-bg, #f6f5f2);
		padding: 0.15rem 0.45rem;
		border-radius: 4px;
		letter-spacing: 0.08em;
		font-weight: 700;

	}

	.ad-title {
		font-family: var(--wf-serif, Georgia, serif);
		font-size: clamp(1.65rem, 4vw, 2.15rem);
		font-weight: 500;
		letter-spacing: -0.02em;
		margin: 0 0 0.65rem;

	}

	.ad-lede {
		margin: 0;
		max-width: 42rem;
		line-height: 1.55;
		color: var(--ink-soft);
		font-size: 0.98rem;

		code {
			font-family: var(--wf-mono, ui-monospace, monospace);
			font-size: 0.88em;
			padding: 0.1em 0.3em;
			border-radius: 4px;
			background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));

		}
	}

	.ad-alert {
		display: flex;
		gap: 0.75rem;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		border-radius: var(--wf-radius, 6px);
		background: rgba(179, 58, 58, 0.08);
		border: 1px solid rgba(179, 58, 58, 0.22);
		color: var(--wf-danger, #b33a3a);
		font-size: 0.92rem;

	}

	.ad-stats {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1.25rem;
		flex-wrap: wrap;

	}

	.ad-stat {
		display: flex;
		align-items: baseline;
		gap: 0.35rem;
		padding: 0.35rem 0.7rem;
		border-radius: 999px;
		background: transparent;
		border: 1px solid var(--edge);

		&-n {
			font-weight: 700;
			font-variant-numeric: tabular-nums;

		}
		&-l {
			font-size: 0.72rem;
			font-weight: 500;
			text-transform: uppercase;
			letter-spacing: 0.03em;
			color: var(--ink-soft);

		}
	}

	.ad-empty {
		color: var(--ink-soft);

	}

	.ad-cap {
		font-size: 0.88rem;
		color: var(--ink-soft);
		margin: 0 0 1rem;
		padding: 0.65rem 0.85rem;
		border-radius: var(--wf-radius, 6px);
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		border: 1px solid var(--edge);

		code {
			font-family: var(--wf-mono, ui-monospace, monospace);
			font-size: 0.9em;

		}
	}

	.ad-table-wrap {
		overflow-x: auto;
		border-radius: var(--df-radius-lg, 10px);
		border: 1px solid var(--edge);
		background: var(--surface);
		box-shadow: none;

	}

	.ad-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.9rem;

		th, td {
			text-align: left;
			padding: 0.65rem 0.85rem;
			border-bottom: 1px solid var(--edge);
			vertical-align: middle;

		}
		th {
			font-size: 0.68rem;
			font-weight: 700;
			letter-spacing: 0.08em;
			text-transform: uppercase;
			color: var(--ink-soft);
			background: rgba(28, 28, 26, 0.03);

		}
		tr:last-child td {
			border-bottom: none;

		}
		tbody tr:hover td {
			background: rgba(28, 28, 26, 0.02);

		}
	}

	.ad-owner {
		font-weight: 600;
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.85em;

	}

	.ad-id {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.78em;
		color: var(--ink-soft);
		max-width: 7rem;
		overflow: hidden;
		text-overflow: ellipsis;

	}

	.ad-status {
		font-size: 0.7rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		padding: 0.15rem 0.45rem;
		border-radius: 999px;
		background: rgba(28, 28, 26, 0.06);
		color: var(--ink-soft);

		&[data-status='open'] {
			background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
			color: var(--wf-accent, #3d5a80);

		}
		&[data-status='completed'] {
			background: rgba(47, 111, 78, 0.12);
			color: var(--wf-success, #2f6f4e);

		}
		&[data-status='archived'] {
			background: rgba(28, 28, 26, 0.06);
			color: var(--wf-ink-muted, #8a8a82);

		}
	}

	.ad-actions {
		white-space: nowrap;

	}

	.ad-btn {
		font: inherit;
		font-size: 0.78rem;
		font-weight: 600;
		border: none;
		border-radius: var(--wf-radius, 6px);
		padding: 0.35rem 0.65rem;
		cursor: pointer;
		background: rgba(179, 58, 58, 0.1);
		color: var(--wf-danger, #b33a3a);

		&:hover:not(:disabled) {
			background: rgba(179, 58, 58, 0.18);

		}
		&:disabled {
			opacity: 0.5;
			cursor: not-allowed;

		}
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

		code {
			font-family: var(--wf-mono, ui-monospace, monospace);
			font-size: 0.9em;

		}
	}
</style>
