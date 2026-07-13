<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Card title */
		title?: string;
		/** Card content */
		children: Snippet;
	}

	let { title, children }: Props = $props();
</script>

<div class="dashboard-card">
	{#if title}
		<header class="card-header">
			<h3>{title}</h3>
		</header>
	{/if}
	<div class="card-body">
		{@render children()}
	</div>
</div>

<style>
.dashboard-card {
	background: var(--hops-bg-white);
	border: 1px solid var(--hops-border);
	border-radius: 12px;
	overflow: hidden;
}

.card-header {
	padding: 0.875rem 1.25rem;
	border-bottom: 1px solid var(--hops-border);
	background: var(--hops-bg-light);
}

.card-header h3 {
	margin: 0;
	font-family: var(--font-display);
	font-size: 0.8rem;
	font-weight: 600;
	color: var(--hops-text-secondary);
	text-transform: uppercase;
	letter-spacing: 0.06em;
}

.card-body {
	padding: 0;
}
</style>
