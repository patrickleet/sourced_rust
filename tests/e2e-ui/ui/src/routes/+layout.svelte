<script lang="ts">
	import '../app.css';
	import type { Snippet } from 'svelte';
	import AuthRefresh from '$lib/components/shared/AuthRefresh.svelte';
	import Navbar from '$lib/components/shared/header/Navbar.svelte';

	let { children }: { children: Snippet } = $props();
</script>

<AuthRefresh />
<Navbar />
<main>
	{@render children()}
</main>

<style>
	main {
		position: relative;
		min-height: 100vh;
	}
</style>
