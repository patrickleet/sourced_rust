<script lang="ts">
	/**
	 * Admin: separate elevated generated surface + causal command runtime.
	 */
	import { AdminAllTodos, useCommands } from '$distributed/admin';
	import { Button } from '$lib/components/shared/ui';
	import {
		AppPage,
		InlineAlert,
		PageHeader,
		StatRow
	} from '$lib/components/product';
	import { HowItsBuilt } from '$lib/components/walkthrough';
	import { adminWalkthrough } from '$lib/walkthrough';
	import { sessionDisplayName } from '$lib/session';

	let { data } = $props();
	let actionError = $state<string | null>(null);
	let busy = $state(false);

	const who = $derived(sessionDisplayName(data.session));
	const listLimit = 100;

	const query = AdminAllTodos.use();
	const commands = useCommands();

	// The generated query/index plan owns collection order.
	const todos = $derived($query.complete ? $query.data.todos : []);
	const owners = $derived([...new Set(todos.map((todo) => todo.owner_id))].sort());
	const open = $derived(todos.filter((todo) => todo.status !== 'archived'));
	const atCap = $derived(todos.length >= listLimit);

	async function forceArchive(todo_id: string) {
		if (busy) return;
		const target = todos.find((todo) => todo.todo_id === todo_id);
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

<AppPage width="wide">
	<PageHeader kicker="admin · Role-scoped GraphQL" title="All todos">
		Signed in as <strong>{who}</strong> with engine role
		<code>{data.engineRole}</code>. This nested layout installs a separate
		<code>e2e-ui-admin</code> client, so elevated query and command artifacts cannot leak
		into the normal application bundle.
	</PageHeader>

	{#if data.gqlError}
		<InlineAlert label="SSR GraphQL">{data.gqlError}</InlineAlert>
	{/if}
	{#if actionError}
		<InlineAlert label="Mutation">{actionError}</InlineAlert>
	{/if}

	<StatRow
		stats={[
			{ value: todos.length, label: 'notes' },
			{ value: owners.length, label: 'owners' },
			{ value: open.length, label: 'active' }
		]}
	/>

	{#if atCap}
		<p class="ad-cap" role="status">
			Showing first {listLimit} notes (bounded admin query). Refine filters or raise limit in
			<code>+page.graphql</code> if needed.
		</p>
	{/if}

	{#if todos.length === 0}
		<p class="ad-empty">No todos in the read model yet. Create some as alice/bob on /todos.</p>
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
					{#each todos as t (t.todo_id)}
						<tr data-todo-id={t.todo_id}>
							<td class="ad-owner">{t.owner_id}</td>
							<td>{t.title}</td>
							<td><span class="ad-status" data-status={t.status}>{t.status}</span></td>
							<td class="ad-id">{t.todo_id}</td>
							<td class="ad-actions">
								{#if t.status !== 'archived'}
									<Button
										type="button"
										variant="ghost"
										size="sm"
										disabled={busy}
										onclick={() => forceArchive(t.todo_id)}
									>
										Force archive
									</Button>
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
		The normal <code>e2e-ui</code> surface cannot even name
		<code>todo.force_archive</code>. The elevated artifact is generated only for this
		admin-gated component tree.
	</p>
</AppPage>

<HowItsBuilt demo={adminWalkthrough} />

<style>
	/* Route-local table surface */
	.ad-table-wrap {
		--ink: var(--wf-ink, #1c1c1a);
		--ink-soft: var(--wf-ink-soft, #5c5c56);
		--edge: var(--wf-line, #e2e0d9);
		--surface: var(--wf-bg-elevated, #fff);
		overflow-x: auto;
		border-radius: var(--df-radius-lg, 10px);
		border: 1px solid var(--edge);
		background: var(--surface);
	}

	.ad-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.9rem;
		color: var(--ink);
	}

	.ad-table th,
	.ad-table td {
		text-align: left;
		padding: 0.65rem 0.85rem;
		border-bottom: 1px solid var(--edge);
		vertical-align: middle;
	}

	.ad-table th {
		font-size: 0.68rem;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--ink-soft);
		background: rgba(28, 28, 26, 0.02);
	}

	.ad-table tr:last-child td {
		border-bottom: none;
	}

	.ad-owner,
	.ad-id {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.78rem;
	}

	.ad-status {
		font-size: 0.75rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.ad-status[data-status='open'] {
		color: var(--wf-accent, #3d5a80);
	}

	.ad-status[data-status='completed'] {
		color: var(--wf-success, #2f6f4e);
	}

	.ad-status[data-status='archived'] {
		color: var(--ink-soft);
	}

	.ad-actions {
		white-space: nowrap;
	}

	.ad-muted {
		color: var(--ink-soft);
		font-size: 0.85rem;
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
		border: 1px solid var(--edge, var(--wf-line, #e2e0d9));
	}

	.ad-cap code,
	.ad-foot code {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.9em;
	}

	.ad-foot {
		margin-top: 1.5rem;
		font-size: 0.88rem;
		line-height: 1.5;
		color: var(--ink-soft, var(--wf-ink-soft, #5c5c56));
	}
</style>
