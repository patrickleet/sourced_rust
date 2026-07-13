<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Page title for <title> tag */
		title?: string;
		/** Meta description */
		description?: string;
		/** Page content */
		children: Snippet;
	}

	let { title, description, children }: Props = $props();

	const fullTitle = $derived(title ? `${title} | Hops` : 'Hops');
</script>

<svelte:head>
	<title>{fullTitle}</title>
	{#if description}
		<meta name="description" content={description} />
	{/if}
</svelte:head>

<div class="page">
	{@render children()}
</div>

<style>
.page {
	min-height: 100vh;
}
</style>
