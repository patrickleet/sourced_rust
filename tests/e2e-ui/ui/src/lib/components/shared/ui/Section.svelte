<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Section id for anchor links */
		id?: string;
		/** Include container wrapper */
		container?: boolean;
		/** Section content */
		children: Snippet;
	}

	let { id, container = false, children }: Props = $props();
</script>

<section {id} class="section">
	{#if container}
		<div class="container">
			{@render children()}
		</div>
	{:else}
		{@render children()}
	{/if}
</section>

<style>
.section {
	padding: var(--section-padding) 0;
	position: relative;
}
</style>
