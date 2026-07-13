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
	background: var(--hops-bg-light);
}

.simple-page-container {
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
.simple-page-container :global(h1) {
	font-family: var(--font-display);
	font-size: clamp(2rem, 5vw, 3rem);
	font-weight: 800;
	color: var(--hops-navy);
	margin-bottom: 1rem;
}

.simple-page-container :global(p) {
	font-size: 1.125rem;
	color: var(--hops-text-secondary);
	margin-bottom: 2rem;
}

.simple-page-container :global(code) {
	font-family: var(--font-mono);
	font-size: 0.9em;
	background: var(--hops-navy);
	color: var(--hops-orange-light);
	padding: 0.2em 0.5em;
	border-radius: 4px;
}
</style>
