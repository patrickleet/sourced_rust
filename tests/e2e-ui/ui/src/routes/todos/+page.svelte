<script lang="ts">
	/**
	 * Field notes — generated operation state + generated causal commands.
	 *
	 * There is no app cache adapter or manual optimistic recipe here. The
	 * compiler artifact tells the replica how command facts affect this query.
	 */
	import { Todos, useCommands } from '$distributed';
	import { sessionDisplayName } from '$lib/session';

	let { data } = $props();

	let title = $state('');
	let actionError = $state<string | null>(null);
	let busy = $state(false);

	const who = $derived(sessionDisplayName(data.session));

	const list = Todos.use();
	const commands = useCommands();

	// The generated query/index plan owns collection order. Components only
	// derive presentation groups from the reactive result.
	const rows = $derived($list.complete ? $list.data.todos : []);
	const open = $derived(rows.filter((todo) => todo.status === 'open'));
	const done = $derived(rows.filter((todo) => todo.status === 'completed'));
	const archived = $derived(rows.filter((todo) => todo.status === 'archived'));

	async function onCreate(e: Event) {
		e.preventDefault();
		const text = title.trim();
		if (!text || busy) return;

		actionError = null;
		busy = true;
		title = '';
		try {
			await commands.todo.create({ title: text });
		} catch (error) {
			actionError = error instanceof Error ? error.message : 'create failed';
		} finally {
			busy = false;
		}
	}

	async function onComplete(todo_id: string) {
		if (busy) return;
		const target = rows.find((todo) => todo.todo_id === todo_id);
		if (!target || target.status !== 'open') return;

		actionError = null;
		busy = true;
		try {
			await commands.todo.complete({ todo_id });
		} catch (error) {
			actionError = error instanceof Error ? error.message : 'complete failed';
		} finally {
			busy = false;
		}
	}

	async function onReopen(todo_id: string) {
		if (busy) return;
		const target = rows.find((todo) => todo.todo_id === todo_id);
		if (!target || target.status !== 'completed') return;

		actionError = null;
		busy = true;
		try {
			await commands.todo.reopen({ todo_id });
		} catch (error) {
			actionError = error instanceof Error ? error.message : 'reopen failed';
		} finally {
			busy = false;
		}
	}

	async function onArchive(todo_id: string) {
		if (busy) return;
		const target = rows.find((todo) => todo.todo_id === todo_id);
		if (!target || target.status === 'archived') return;

		actionError = null;
		busy = true;
		try {
			await commands.todo.archive({ todo_id });
		} catch (error) {
			actionError = error instanceof Error ? error.message : 'archive failed';
		} finally {
			busy = false;
		}
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
			Tasks for <strong>{who}</strong>. One generated <code>@load</code> operation feeds
			SSR, navigation, and cache reads; typed commands update that same state
			optimistically and retire it when projection catches up.
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
								<button
									class="fn-check"
									type="button"
									title="Mark done"
									aria-label="Mark done: {t.title}"
									disabled={busy}
									onclick={() => onComplete(t.todo_id)}
								></button>
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
								<button
									class="fn-check fn-check-on"
									type="button"
									title="Reopen"
									aria-label="Reopen: {t.title}"
									disabled={busy}
									onclick={() => onReopen(t.todo_id)}
								>
									<svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
										<path
											d="M5 12l5 5L20 7"
											stroke="currentColor"
											stroke-width="3"
											stroke-linecap="round"
											stroke-linejoin="round"
										/>
									</svg>
								</button>
								<span class="fn-item-title">{t.title}</span>
							</div>
							<div class="fn-item-actions">
								<button
									class="fn-btn fn-btn-ghost"
									type="button"
									title="Reopen"
									disabled={busy}
									onclick={() => onReopen(t.todo_id)}
								>
									Reopen
								</button>
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
		--surface: var(--wf-bg-elevated, #fff);
		--surface-edge: var(--wf-line, #e2e0d9);
		--ink: var(--wf-ink, #1c1c1a);
		--ink-soft: var(--wf-ink-soft, #5c5c56);
		--accent: var(--wf-accent, #3d5a80);
		--shadow: var(--df-shadow-sm, 0 1px 2px rgba(28, 28, 26, 0.04));

		position: relative;
		max-width: 52rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 4rem;
		font-family: var(--wf-sans, var(--font-body, system-ui, sans-serif));
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
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--ink-soft);
		margin-bottom: 0.65rem;
	}

	.fn-dot {
		width: 0.4rem;
		height: 0.4rem;
		border-radius: 50%;
		background: var(--accent);
	}

	.fn-title {
		font-family: var(--wf-serif, Georgia, serif);
		font-size: clamp(1.65rem, 4vw, 2.15rem);
		font-weight: 500;
		letter-spacing: -0.02em;
		line-height: 1.1;
		margin: 0 0 0.65rem;
		color: var(--ink);
	}

	.fn-lede {
		margin: 0;
		max-width: 36rem;
		font-size: 1rem;
		line-height: 1.55;
		color: var(--ink-soft);
	}

	.fn-lede code {
		font-family: var(--wf-mono, var(--font-mono, ui-monospace, monospace));
		font-size: 0.88em;
		padding: 0.1em 0.35em;
		border-radius: 4px;
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
	}

	.fn-alert {
		display: flex;
		gap: 0.75rem;
		align-items: flex-start;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		border-radius: var(--wf-radius, 6px);
		background: rgba(179, 58, 58, 0.08);
		border: 1px solid rgba(179, 58, 58, 0.22);
		color: var(--wf-danger, #b33a3a);
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
		padding: 0.65rem 0.75rem;
		margin-bottom: 1.25rem;
		background: var(--surface);
		border: 1px solid var(--surface-edge);
		border-radius: var(--df-radius-lg, 10px);
		box-shadow: var(--shadow);
	}

	.fn-input {
		flex: 1;
		min-width: 12rem;
		border: none;
		background: transparent;
		padding: 0.55rem 0.65rem;
		font: inherit;
		font-size: 1rem;
		color: var(--ink);
		outline: none;
	}

	.fn-input::placeholder {
		color: var(--wf-ink-muted, #8a8a82);
	}

	.fn-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 0.4rem;
		border: none;
		font: inherit;
		font-weight: 600;
		font-size: 0.9rem;
		cursor: pointer;
		border-radius: var(--wf-radius, 6px);
		padding: 0.55rem 0.95rem;
		transition: background 0.15s ease, color 0.15s ease;
	}

	.fn-btn-primary {
		background: var(--ink);
		color: #fff;
	}

	.fn-btn-primary:hover:not(:disabled) {
		background: var(--hops-navy-light, #2a2a28);
	}

	.fn-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.fn-btn-ghost {
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		color: var(--accent);
		padding: 0.35rem 0.7rem;
		font-size: 0.8rem;
	}

	.fn-btn-ghost:hover:not(:disabled) {
		background: rgba(61, 90, 128, 0.14);
	}

	.fn-btn-quiet {
		background: transparent;
		color: var(--ink-soft);
		padding: 0.35rem 0.6rem;
		font-size: 0.8rem;
		font-weight: 500;
	}

	.fn-btn-quiet:hover:not(:disabled) {
		background: rgba(28, 28, 26, 0.05);
		color: var(--ink);
	}

	.fn-stats {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1.25rem;
		flex-wrap: wrap;
	}

	.fn-stat {
		display: flex;
		align-items: baseline;
		gap: 0.35rem;
		padding: 0.35rem 0.7rem;
		border-radius: 999px;
		background: transparent;
		border: 1px solid var(--surface-edge);
	}

	.fn-stat-n {
		font-weight: 700;
		font-size: 0.95rem;
		font-variant-numeric: tabular-nums;
		color: var(--ink);
	}

	.fn-stat-l {
		font-size: 0.72rem;
		font-weight: 500;
		letter-spacing: 0.03em;
		text-transform: uppercase;
		color: var(--ink-soft);
	}

	.fn-board {
		display: grid;
		gap: 1rem;
	}

	@media (min-width: 768px) {
		.fn-board {
			grid-template-columns: 1fr 1fr;
			align-items: start;
		}
	}

	.fn-panel {
		background: var(--surface);
		border: 1px solid var(--surface-edge);
		border-radius: var(--df-radius-lg, 10px);
		padding: 1rem 1.05rem 0.75rem;
		box-shadow: none;
	}

	.fn-panel-muted {
		background: var(--surface);
	}

	.fn-panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 0.65rem;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--surface-edge);
	}

	.fn-panel-head h2 {
		margin: 0;
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--ink-soft);
	}

	.fn-count {
		font-variant-numeric: tabular-nums;
		font-weight: 600;
		font-size: 0.75rem;
		min-width: 1.4rem;
		text-align: center;
		padding: 0.1rem 0.4rem;
		border-radius: 999px;
		background: var(--ink);
		color: #fff;
	}

	.fn-empty {
		margin: 0.5rem 0 0.75rem;
		font-size: 0.9rem;
		color: var(--ink-soft);
	}

	.fn-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}

	.fn-item {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem 0.75rem;
		padding: 0.5rem 0.3rem;
		border-radius: var(--wf-radius, 6px);
		transition: background 0.12s ease;
	}

	.fn-item:hover {
		background: rgba(28, 28, 26, 0.04);
	}

	.fn-item-main {
		display: flex;
		align-items: flex-start;
		gap: 0.6rem;
		flex: 1;
		min-width: 0;
	}

	.fn-check {
		flex-shrink: 0;
		display: grid;
		place-items: center;
		width: 1.15rem;
		height: 1.15rem;
		margin-top: 0.1rem;
		padding: 0;
		border-radius: 4px;
		border: 1.5px solid var(--wf-line-strong, #cdcabe);
		background: var(--surface);
		color: inherit;
		cursor: pointer;
		appearance: none;
		transition:
			border-color 0.12s ease,
			background 0.12s ease;
	}

	.fn-check:hover:not(:disabled) {
		border-color: var(--accent);
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
	}

	.fn-check:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.fn-check-on {
		border-color: var(--wf-success, #2f6f4e);
		background: rgba(47, 111, 78, 0.1);
		color: var(--wf-success, #2f6f4e);
	}

	.fn-check-on:hover:not(:disabled) {
		border-color: var(--wf-ink-soft, #5c5c56);
		background: rgba(28, 28, 26, 0.06);
		color: var(--ink-soft);
	}

	.fn-item-title {
		font-size: 0.95rem;
		font-weight: 450;
		line-height: 1.4;
		word-break: break-word;
	}

	.fn-item-done .fn-item-title {
		text-decoration: line-through;
		text-decoration-thickness: 1px;
		color: var(--ink-soft);
	}

	.fn-item-actions {
		display: flex;
		gap: 0.2rem;
		flex-shrink: 0;
	}

	.fn-archive {
		margin-top: 1.25rem;
		padding: 0.75rem 0.95rem;
		border-radius: var(--df-radius-lg, 10px);
		border: 1px solid var(--surface-edge);
		background: var(--surface);
	}

	.fn-archive summary {
		cursor: pointer;
		font-weight: 600;
		font-size: 0.8rem;
		letter-spacing: 0.03em;
		text-transform: uppercase;
		color: var(--ink-soft);
		list-style: none;
	}

	.fn-archive summary::-webkit-details-marker {
		display: none;
	}

	.fn-list-compact {
		margin-top: 0.65rem;
	}

	.fn-item-archived {
		opacity: 0.7;
		justify-content: flex-start;
		gap: 0.75rem;
	}

	.fn-badge {
		font-size: 0.65rem;
		font-weight: 600;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		padding: 0.12rem 0.4rem;
		border-radius: 999px;
		background: rgba(28, 28, 26, 0.06);
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
