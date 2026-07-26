<script lang="ts">
	import type { Snippet } from 'svelte';
	import Page from './Page.svelte';

	interface Props {
		/** Page title for <title> tag */
		title?: string;
		/** Meta description */
		description?: string;
		/** Maximum container width */
		maxWidth?: 'sm' | 'md' | 'lg';
		/** Page content */
		children: Snippet;
	}

	let {
		title,
		description,
		maxWidth = 'sm',
		children
	}: Props = $props();
</script>

<Page {title} {description}>
	<div class="simple-page">
		<div class="simple-page-container max-{maxWidth}">
			{@render children()}
		</div>
	</div>
</Page>

<style>
	.simple-page {
		padding: 7rem 0 4rem;
		min-height: 100vh;
		background: var(--wf-bg, var(--hops-bg-light));
		&-container {
			width: 100%;
			margin: 0 auto;
			padding: 0 2rem;
			text-align: center;

			@media (--tablet) {
				padding: 0 1.5rem;
			}

			@media (--mobile) {
				padding: 0 1rem;
			}
			:global(h1) {
				font-family: var(--wf-serif, var(--font-display));
				font-size: clamp(1.65rem, 4vw, 2.15rem);
				font-weight: 500;
				letter-spacing: -0.02em;
				color: var(--wf-ink, var(--hops-navy));
				margin-bottom: 0.85rem;
			}
			:global(p) {
				font-size: 1.02rem;
				color: var(--wf-ink-soft, var(--hops-text-secondary));
				margin-bottom: 1.75rem;
			}
			:global(code) {
				font-family: var(--wf-mono, var(--font-mono));
				font-size: 0.88em;
				background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
				color: var(--wf-ink, #1c1c1a);
				padding: 0.15em 0.4em;
				border-radius: 4px;
			}
		}
	}

	.max-sm {
		max-width: 600px;
	}

	.max-md {
		max-width: 900px;
	}

	.max-lg {
		max-width: 1100px;
	}

	/* Typography defaults for simple pages */
</style>
