<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Optional panel title */
		title?: string;
		/** Count badge next to title */
		count?: number | string;
		/** Muted surface */
		muted?: boolean;
		children: Snippet;
	}

	let { title, count, muted = false, children }: Props = $props();
</script>

<section class="panel" class:muted>
	{#if title !== undefined}
		<div class="head">
			<h2>{title}</h2>
			{#if count !== undefined}
				<span class="count">{count}</span>
			{/if}
		</div>
	{/if}
	{@render children()}
</section>

<style>
	.panel {
		background: var(--wf-bg-elevated, #fff);
		border: 1px solid var(--wf-line, #e2e0d9);
		border-radius: var(--df-radius-lg, 10px);
		padding: 1rem 1.05rem 0.75rem;
	}

	.panel.muted {
		background: var(--wf-bg-elevated, #fff);
	}

	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 0.65rem;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--wf-line, #e2e0d9);
	}

	.head h2 {
		margin: 0;
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--wf-ink-soft, #5c5c56);
	}

	.count {
		font-variant-numeric: tabular-nums;
		font-weight: 600;
		font-size: 0.75rem;
		min-width: 1.4rem;
		text-align: center;
		padding: 0.1rem 0.4rem;
		border-radius: 999px;
		background: var(--wf-ink, #1c1c1a);
		color: #fff;
	}
</style>
