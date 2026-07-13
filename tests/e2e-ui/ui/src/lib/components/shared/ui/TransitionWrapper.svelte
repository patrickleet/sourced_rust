<script lang="ts">
	import { fly } from 'svelte/transition';
	import { cubicOut } from 'svelte/easing';
	import { page } from '$app/state';
	import type { Snippet } from 'svelte';

	interface Props {
		/** Page content */
		children: Snippet;
	}

	let { children }: Props = $props();

	// Use pathname as key for transitions
	const key = $derived(page.url.pathname);
</script>

{#key key}
	<div
		class="page"
		in:fly={{ y: 20, duration: 300, delay: 150, easing: cubicOut }}
		out:fly={{ y: -10, duration: 150, easing: cubicOut }}
	>
		{@render children()}
	</div>
{/key}

<style>
.page {
	min-height: 100vh;
	will-change: transform, opacity;
}
</style>
