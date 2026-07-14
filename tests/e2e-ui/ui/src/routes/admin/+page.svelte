<script lang="ts">
	/**
	 * Admin: all field notes across owners.
	 * GraphQL: same `todos` field as /todos; role `admin` has no owner filter.
	 */
	import { sessionDisplayName } from '$lib/session';

	let { data } = $props();

	const who = $derived(sessionDisplayName(data.session));
	const todos = $derived(data.todos ?? []);
	const owners = $derived([...new Set(todos.map((t) => t.owner_id))].sort());
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
			<code>{data.engineRole}</code>. This route is UI-gated to
			<code>admin</code>; the query is the same
			<code>todos</code> field as personal notes, but
			<code>ModelPermissions</code> omit the
			<code>owner_id = claim(x-user-id)</code> filter for admins — so every
			owner appears below.
		</p>
	</header>

	{#if data.gqlError}
		<div class="ad-alert" role="alert">
			<strong>SSR GraphQL</strong>
			<span>{data.gqlError}</span>
		</div>
	{/if}

	<div class="ad-stats">
		<div class="ad-stat">
			<span class="ad-stat-n">{todos.length}</span>
			<span class="ad-stat-l">notes</span>
		</div>
		<div class="ad-stat">
			<span class="ad-stat-n">{owners.length}</span>
			<span class="ad-stat-l">owners</span>
		</div>
	</div>

	{#if todos.length === 0}
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
					</tr>
				</thead>
				<tbody>
					{#each todos as t (t.todo_id)}
						<tr>
							<td class="ad-owner">{t.owner_id}</td>
							<td>{t.title}</td>
							<td><span class="ad-status" data-status={t.status}>{t.status}</span></td>
							<td class="ad-id">{t.todo_id}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<p class="ad-foot">
		Compare: as <code>user</code>, <code>&#123; todos &#123; owner_id &#125; &#125;</code> only
		returns your rows (suite T2). As <code>admin</code>, the same selection returns every owner.
	</p>
</section>

<style>
	.ad-page {
		--ink: var(--hops-navy, #1a2744);
		--ink-soft: rgba(26, 39, 68, 0.62);
		max-width: 52rem;
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
		max-width: 40rem;
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
		max-width: 8rem;
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
