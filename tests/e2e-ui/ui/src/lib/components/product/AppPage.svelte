<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Constrain width (default product measure) */
		width?: 'default' | 'narrow' | 'wide';
		children: Snippet;
	}

	let { width = 'default', children }: Props = $props();
</script>

<section class="app-page width-{width}">
	{@render children()}
</section>

<style>
	.app-page {
		--surface: var(--wf-bg-elevated, #fff);
		--surface-edge: var(--wf-line, #e2e0d9);
		--ink: var(--wf-ink, #1c1c1a);
		--ink-soft: var(--wf-ink-soft, #5c5c56);
		--accent: var(--wf-accent, #3d5a80);
		--shadow: var(--df-shadow-sm, 0 1px 2px rgba(28, 28, 26, 0.04));

		position: relative;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 4rem;
		font-family: var(--wf-sans, var(--font-body, system-ui, sans-serif));
		color: var(--ink);
	}

	.width-default {
		max-width: 52rem;
	}

	.width-narrow {
		max-width: 40rem;
	}

	.width-wide {
		max-width: 68rem;
	}
</style>
