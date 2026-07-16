<script lang="ts">
	/**
	 * Field notes — document store + command pipeline.
	 * Writes: optimistic → network → fact; no immediate refetch (async projectors).
	 * Soft delayed refetch only so cache eventually matches RM.
	 */
	import { onDestroy } from 'svelte';
	import { useGraphql, fx } from '$lib/gql';
	import { sessionDisplayName } from '$lib/session';
	import { todos as todosResource } from './todos.resource';
	import type { TodoRow } from './todos.resource';

	type Todo = TodoRow;

	let { data } = $props();

	let title = $state('');
	let actionError = $state<string | null>(null);
	let busy = $state(false);

	const me = $derived(data.session?.user?.id ?? '');
	const who = $derived(sessionDisplayName(data.session));

	const gql = useGraphql(() => data, {
		runEffects: (effects) => {
			for (const e of effects) {
				if (e.kind === 'alert') actionError = e.message;
			}
		}
	});

	function sortTodos(list: Todo[]): Todo[] {
		const rank = (s: string) => (s === 'open' ? 0 : s === 'completed' ? 1 : 2);
		return [...list].sort((a, b) => {
			const r = rank(a.status) - rank(b.status);
			if (r !== 0) return r;
			return b.todo_id.localeCompare(a.todo_id);
		});
	}

	const list = gql.store({
		document: todosResource.query,
		initialData: { todos: data.todos ?? [] },
		select: (d: { todos?: Todo[] }) => sortTodos(d?.todos ?? [])
	});

	$effect(() => {
		list.seed({ todos: data.todos ?? [] });
	});

	onDestroy(() => list.destroy());

	const open = $derived($list.data.filter((t) => t.status === 'open'));
	const done = $derived($list.data.filter((t) => t.status === 'completed'));
	const archived = $derived($list.data.filter((t) => t.status === 'archived'));

	function newTodoId() {
		const rand =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID().replace(/-/g, '').slice(0, 12)
				: `${Date.now().toString(16)}${Math.random().toString(16).slice(2, 8)}`;
		return `t-${rand}`;
	}

	// Never reconcile: 'refetch' on the command path — RM is not ready yet.
	const factOpts = {
		result: { kind: 'fact' as const },
		reconcile: { kind: 'none' as const }
	};

	/** After projector lag, refresh cache (merge preserves optimistic if still ahead). */
	function scheduleProjectorCatchUp() {
		window.setTimeout(() => {
			void list.refetch();
		}, 800);
	}

	async function onCreate(e: Event) {
		e.preventDefault();
		const text = title.trim();
		if (!text || busy) return;

		const todo_id = newTodoId();
		actionError = null;
		busy = true;
		title = '';

		const result = await gql.commands.todosCreate(
			{ todo_id, title: text },
			{
				...factOpts,
				optimistic: {
					targets: [list.target('todos', 'todo_id')],
					row: {
						todo_id,
						owner_id: me || 'me',
						title: text,
						status: 'open'
					}
				},
				onError: ({ errors }) => [fx.alert(errors[0]?.message ?? 'create failed')]
			}
		);

		busy = false;
		if (result.errors?.length || !result.data) {
			if (!actionError) actionError = result.errors?.[0]?.message ?? 'create failed';
			return;
		}
		scheduleProjectorCatchUp();
	}

	async function onComplete(todo_id: string) {
		if (busy) return;
		const target = $list.data.find((t) => t.todo_id === todo_id);
		if (!target || target.status !== 'open') return;

		actionError = null;
		busy = true;

		const result = await gql.commands.todosComplete(
			{ todo_id },
			{
				...factOpts,
				optimistic: {
					targets: [list.target('todos', 'todo_id')],
					row: { ...target, status: 'completed' }
				},
				onError: ({ errors }) => [fx.alert(errors[0]?.message ?? 'complete failed')]
			}
		);

		busy = false;
		if (result.errors?.length || !result.data) {
			if (!actionError) actionError = result.errors?.[0]?.message ?? 'complete failed';
			return;
		}
		scheduleProjectorCatchUp();
	}

	async function onArchive(todo_id: string) {
		if (busy) return;
		const target = $list.data.find((t) => t.todo_id === todo_id);
		if (!target || target.status === 'archived') return;

		actionError = null;
		busy = true;

		const result = await gql.commands.todosArchive(
			{ todo_id },
			{
				...factOpts,
				optimistic: {
					targets: [list.target('todos', 'todo_id')],
					row: { ...target, status: 'archived' }
				},
				onError: ({ errors }) => [fx.alert(errors[0]?.message ?? 'archive failed')]
			}
		);

		busy = false;
		if (result.errors?.length || !result.data) {
			if (!actionError) actionError = result.errors?.[0]?.message ?? 'archive failed';
			return;
		}
		scheduleProjectorCatchUp();
	}
</script>

<section class="fn-page">
	<header class="fn-header">
		<div class="fn-kicker">
			<span class="fn-dot" aria-hidden="true"></span>
			Personal · owner-scoped
		</div>
		<h1 class="fn-title">Field notes</h1>
		<p class="fn-lede">
			Tasks for <strong>{who}</strong>. List via <code>gql.store</code>
			(<code>$list.data</code>); writes via <code>gql.commands.*</code>
			(optimistic → fact, no immediate refetch).
		</p>
	</header>

	{#if data.gqlError}
		<div class="fn-alert" role="alert">
			<span class="fn-alert-label">SSR GraphQL</span>
			{data.gqlError}
		</div>
	{/if}
	{#if actionError}
		<div class="fn-alert" role="alert">
			<span class="fn-alert-label">Mutation</span>
			{actionError}
		</div>
	{/if}

	<form class="fn-composer" onsubmit={onCreate}>
		<label class="fn-sr" for="todo-title">New task</label>
		<input
			id="todo-title"
			class="fn-input"
			name="title"
			placeholder="Capture something that needs doing…"
			required
			autocomplete="off"
			bind:value={title}
		/>
		<button class="fn-btn fn-btn-primary" type="submit" disabled={!title.trim() || busy}>
			<span>Add</span>
			<svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
				<path
					d="M12 5v14M5 12h14"
					stroke="currentColor"
					stroke-width="2.5"
					stroke-linecap="round"
				/>
			</svg>
		</button>
	</form>

	<div class="fn-stats">
		<div class="fn-stat">
			<span class="fn-stat-n">{open.length}</span>
			<span class="fn-stat-l">open</span>
		</div>
		<div class="fn-stat">
			<span class="fn-stat-n">{done.length}</span>
			<span class="fn-stat-l">done</span>
		</div>
		<div class="fn-stat">
			<span class="fn-stat-n">{archived.length}</span>
			<span class="fn-stat-l">archived</span>
		</div>
	</div>

	<div class="fn-board">
		<section class="fn-panel" style="--stagger: 0">
			<div class="fn-panel-head">
				<h2>Open</h2>
				<span class="fn-count">{open.length}</span>
			</div>
			{#if open.length === 0}
				<p class="fn-empty">Nothing open — write one above.</p>
			{:else}
				<ul class="fn-list">
					{#each open as t, i (t.todo_id)}
						<li class="fn-item" style="--i: {i}">
							<div class="fn-item-main">
								<span class="fn-check" aria-hidden="true"></span>
								<span class="fn-item-title">{t.title}</span>
							</div>
							<div class="fn-item-actions">
								<button
									class="fn-btn fn-btn-ghost"
									type="button"
									title="Mark done"
									disabled={busy}
									onclick={() => onComplete(t.todo_id)}
								>
									Done
								</button>
								<button
									class="fn-btn fn-btn-quiet"
									type="button"
									title="Archive"
									disabled={busy}
									onclick={() => onArchive(t.todo_id)}
								>
									Archive
								</button>
							</div>
						</li>
					{/each}
				</ul>
			{/if}
		</section>

		<section class="fn-panel fn-panel-muted" style="--stagger: 1">
			<div class="fn-panel-head">
				<h2>Done</h2>
				<span class="fn-count">{done.length}</span>
			</div>
			{#if done.length === 0}
				<p class="fn-empty">Completed tasks land here.</p>
			{:else}
				<ul class="fn-list">
					{#each done as t, i (t.todo_id)}
						<li class="fn-item fn-item-done" style="--i: {i}">
							<div class="fn-item-main">
								<span class="fn-check fn-check-on" aria-hidden="true">
									<svg width="12" height="12" viewBox="0 0 24 24" fill="none">
										<path
											d="M5 12l5 5L20 7"
											stroke="currentColor"
											stroke-width="3"
											stroke-linecap="round"
											stroke-linejoin="round"
										/>
									</svg>
								</span>
								<span class="fn-item-title">{t.title}</span>
							</div>
							<div class="fn-item-actions">
								<button
									class="fn-btn fn-btn-quiet"
									type="button"
									disabled={busy}
									onclick={() => onArchive(t.todo_id)}
								>
									Archive
								</button>
							</div>
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	</div>

	{#if archived.length}
		<details class="fn-archive">
			<summary>Archived ({archived.length})</summary>
			<ul class="fn-list fn-list-compact">
				{#each archived as t (t.todo_id)}
					<li class="fn-item fn-item-archived">
						<span class="fn-item-title">{t.title}</span>
						<span class="fn-badge">archived</span>
					</li>
				{/each}
			</ul>
		</details>
	{/if}
</section>

<style>
	.fn-page {
		--paper: #fbf8f1;
		--paper-edge: #e8e0d0;
		--ink: var(--hops-navy, #1a2744);
		--ink-soft: rgba(26, 39, 68, 0.62);
		--amber: var(--hops-orange, #e69a2d);
		--amber-glow: rgba(230, 154, 45, 0.22);
		--rule: rgba(26, 39, 68, 0.07);
		--shadow: 0 18px 50px rgba(15, 24, 41, 0.12), 0 2px 0 rgba(255, 255, 255, 0.6) inset;

		position: relative;
		max-width: 52rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 4rem;
		font-family: var(--font-body, 'Lexend', system-ui, sans-serif);
		color: var(--ink);
	}

	.fn-header {
		margin-bottom: 1.75rem;
	}

	.fn-kicker {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--ink-soft);
		margin-bottom: 0.65rem;
	}

	.fn-dot {
		width: 0.45rem;
		height: 0.45rem;
		border-radius: 50%;
		background: var(--amber);
		box-shadow: 0 0 0 4px var(--amber-glow);
	}

	.fn-title {
		font-family: var(--font-display, 'Lexend', system-ui, sans-serif);
		font-size: clamp(2rem, 5vw, 2.75rem);
		font-weight: 800;
		letter-spacing: -0.035em;
		line-height: 1.05;
		margin: 0 0 0.65rem;
		color: var(--ink);
	}

	.fn-lede {
		margin: 0;
		max-width: 36rem;
		font-size: 1.02rem;
		line-height: 1.55;
		color: var(--ink-soft);
	}

	.fn-lede code {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.88em;
		padding: 0.1em 0.35em;
		border-radius: 4px;
		background: rgba(26, 39, 68, 0.06);
	}

	.fn-alert {
		display: flex;
		gap: 0.75rem;
		align-items: flex-start;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		border-radius: 12px;
		background: rgba(229, 62, 62, 0.08);
		border: 1px solid rgba(229, 62, 62, 0.25);
		color: #9b2c2c;
		font-size: 0.92rem;
	}

	.fn-alert-label {
		font-weight: 700;
		font-size: 0.7rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		opacity: 0.8;
		padding-top: 0.15rem;
	}

	.fn-composer {
		display: flex;
		gap: 0.65rem;
		flex-wrap: wrap;
		padding: 0.85rem;
		margin-bottom: 1.25rem;
		background: var(--paper);
		border: 1px solid var(--paper-edge);
		border-radius: 16px;
		box-shadow: var(--shadow);
		position: relative;
		overflow: hidden;
	}

	.fn-composer::before {
		content: '';
		position: absolute;
		inset: 0 auto 0 0;
		width: 4px;
		background: linear-gradient(180deg, var(--amber), var(--hops-orange-dark, #c47f1a));
	}

	.fn-input {
		flex: 1;
		min-width: 12rem;
		border: none;
		background: transparent;
		padding: 0.7rem 0.85rem 0.7rem 1rem;
		font: inherit;
		font-size: 1.05rem;
		color: var(--ink);
		outline: none;
	}

	.fn-input::placeholder {
		color: rgba(26, 39, 68, 0.38);
	}

	.fn-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 0.4rem;
		border: none;
		font: inherit;
		font-weight: 700;
		font-size: 0.9rem;
		cursor: pointer;
		border-radius: 10px;
		padding: 0.65rem 1.05rem;
		transition:
			transform 0.15s var(--ease-out-expo, ease),
			background 0.15s ease,
			box-shadow 0.15s ease;
	}

	.fn-btn:active {
		transform: scale(0.97);
	}

	.fn-btn-primary {
		background: var(--ink);
		color: #fff;
		box-shadow: 0 6px 16px rgba(26, 39, 68, 0.22);
	}

	.fn-btn-primary:hover:not(:disabled) {
		background: var(--hops-navy-light, #2a3a5c);
		box-shadow: 0 8px 22px rgba(26, 39, 68, 0.28);
	}

	.fn-btn:disabled {
		opacity: 0.55;
		cursor: not-allowed;
	}

	.fn-btn-ghost {
		background: rgba(230, 154, 45, 0.14);
		color: var(--hops-orange-dark, #c47f1a);
		padding: 0.4rem 0.75rem;
		font-size: 0.8rem;
	}

	.fn-btn-ghost:hover:not(:disabled) {
		background: rgba(230, 154, 45, 0.28);
	}

	.fn-btn-quiet {
		background: transparent;
		color: var(--ink-soft);
		padding: 0.4rem 0.65rem;
		font-size: 0.8rem;
		font-weight: 600;
	}

	.fn-btn-quiet:hover:not(:disabled) {
		background: rgba(26, 39, 68, 0.06);
		color: var(--ink);
	}

	.fn-stats {
		display: flex;
		gap: 0.75rem;
		margin-bottom: 1.35rem;
		flex-wrap: wrap;
	}

	.fn-stat {
		display: flex;
		align-items: baseline;
		gap: 0.4rem;
		padding: 0.45rem 0.85rem;
		border-radius: 999px;
		background: rgba(26, 39, 68, 0.04);
		border: 1px solid var(--hops-border, rgba(26, 39, 68, 0.1));
	}

	.fn-stat-n {
		font-weight: 800;
		font-size: 1.05rem;
		font-variant-numeric: tabular-nums;
		color: var(--ink);
	}

	.fn-stat-l {
		font-size: 0.75rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--ink-soft);
	}

	.fn-board {
		display: grid;
		gap: 1.15rem;
	}

	@media (min-width: 768px) {
		.fn-board {
			grid-template-columns: 1fr 1fr;
			align-items: start;
		}
	}

	.fn-panel {
		background: var(--paper);
		border: 1px solid var(--paper-edge);
		border-radius: 18px;
		padding: 1.1rem 1.15rem 0.85rem;
		box-shadow: var(--shadow);
		background-image: repeating-linear-gradient(
			transparent,
			transparent 1.85rem,
			var(--rule) 1.85rem,
			var(--rule) calc(1.85rem + 1px)
		);
		background-position: 0 3.2rem;
	}

	.fn-panel-muted {
		opacity: 0.96;
		filter: saturate(0.92);
	}

	.fn-panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 0.85rem;
		padding-bottom: 0.55rem;
		border-bottom: 2px solid rgba(26, 39, 68, 0.08);
	}

	.fn-panel-head h2 {
		margin: 0;
		font-size: 0.78rem;
		font-weight: 800;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--ink);
	}

	.fn-count {
		font-variant-numeric: tabular-nums;
		font-weight: 700;
		font-size: 0.8rem;
		min-width: 1.5rem;
		text-align: center;
		padding: 0.15rem 0.45rem;
		border-radius: 999px;
		background: var(--ink);
		color: #fff;
	}

	.fn-empty {
		margin: 0.5rem 0 0.75rem;
		font-size: 0.92rem;
		color: var(--ink-soft);
		font-style: italic;
	}

	.fn-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}

	.fn-item {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem 0.75rem;
		padding: 0.55rem 0.35rem;
		border-radius: 10px;
		transition: background 0.15s ease;
	}

	.fn-item:hover {
		background: rgba(255, 255, 255, 0.55);
	}

	.fn-item-main {
		display: flex;
		align-items: flex-start;
		gap: 0.65rem;
		flex: 1;
		min-width: 0;
	}

	.fn-check {
		flex-shrink: 0;
		width: 1.15rem;
		height: 1.15rem;
		margin-top: 0.15rem;
		border-radius: 6px;
		border: 2px solid rgba(26, 39, 68, 0.28);
		background: #fff;
	}

	.fn-check-on {
		display: grid;
		place-items: center;
		border-color: var(--hops-success, #38a169);
		background: rgba(56, 161, 105, 0.12);
		color: var(--hops-success, #38a169);
	}

	.fn-item-title {
		font-size: 0.98rem;
		font-weight: 500;
		line-height: 1.4;
		word-break: break-word;
	}

	.fn-item-done .fn-item-title {
		text-decoration: line-through;
		text-decoration-thickness: 1.5px;
		color: var(--ink-soft);
	}

	.fn-item-actions {
		display: flex;
		gap: 0.25rem;
		flex-shrink: 0;
	}

	.fn-archive {
		margin-top: 1.5rem;
		padding: 0.85rem 1rem;
		border-radius: 14px;
		border: 1px dashed rgba(26, 39, 68, 0.18);
		background: rgba(255, 255, 255, 0.4);
	}

	.fn-archive summary {
		cursor: pointer;
		font-weight: 700;
		font-size: 0.85rem;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--ink-soft);
		list-style: none;
	}

	.fn-archive summary::-webkit-details-marker {
		display: none;
	}

	.fn-list-compact {
		margin-top: 0.75rem;
	}

	.fn-item-archived {
		opacity: 0.7;
		justify-content: flex-start;
		gap: 0.75rem;
	}

	.fn-badge {
		font-size: 0.65rem;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		padding: 0.15rem 0.45rem;
		border-radius: 999px;
		background: rgba(26, 39, 68, 0.08);
		color: var(--ink-soft);
	}

	.fn-sr {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		border: 0;
	}
</style>
