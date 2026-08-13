<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Small uppercase kicker above the title */
		kicker?: string;
		/** Page title */
		title: string;
		/** Optional status / meta slot (e.g. live indicator) */
		meta?: Snippet;
		/** Lede / description */
		children?: Snippet;
	}

	let { kicker, title, meta, children }: Props = $props();
</script>

<header class="page-header">
	<div class="title-row">
		<div class="titles">
			{#if kicker}
				<div class="kicker">
					<span class="dot" aria-hidden="true"></span>
					{kicker}
				</div>
			{/if}
			<h1 class="title">{title}</h1>
		</div>
		{#if meta}
			<div class="meta">
				{@render meta()}
			</div>
		{/if}
	</div>
	{#if children}
		<p class="lede">
			{@render children()}
		</p>
	{/if}
</header>

<style>
	.page-header {
		margin-bottom: 1.75rem;
	}

	.title-row {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		justify-content: space-between;
		gap: 1rem;
	}

	.kicker {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		font-size: 0.72rem;
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--wf-ink-soft, #5c5c56);
		margin-bottom: 0.65rem;
	}

	.dot {
		width: 0.4rem;
		height: 0.4rem;
		border-radius: 50%;
		background: var(--wf-accent, #3d5a80);
	}

	.title {
		font-family: var(--wf-serif, Georgia, serif);
		font-size: clamp(1.65rem, 4vw, 2.15rem);
		font-weight: 500;
		letter-spacing: -0.02em;
		line-height: 1.1;
		margin: 0 0 0.65rem;
		color: var(--wf-ink, #1c1c1a);
	}

	.lede {
		margin: 0;
		max-width: 36rem;
		font-size: 1rem;
		line-height: 1.55;
		color: var(--wf-ink-soft, #5c5c56);
	}

	.lede :global(code) {
		font-family: var(--wf-mono, var(--font-mono, ui-monospace, monospace));
		font-size: 0.88em;
		padding: 0.1em 0.35em;
		border-radius: 4px;
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
	}

	.lede :global(strong) {
		color: var(--wf-ink, #1c1c1a);
		font-weight: 600;
	}

	.meta {
		flex-shrink: 0;
	}
</style>
