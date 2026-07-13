<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Adapts to dark backgrounds when true */
		onDark?: boolean;
		/** Label content */
		children: Snippet;
	}

	let { onDark = false, children }: Props = $props();
</script>

<span class="section-label" class:on-dark={onDark}>
	{@render children()}
</span>

<style>
.section-label {
	display: inline-block;
	font-family: var(--font-body);
	font-size: 0.75rem;
	font-weight: 700;
	text-transform: uppercase;
	letter-spacing: 0.15em;
	color: var(--hops-orange);
	background: rgba(230, 154, 45, 0.1);
	padding: 0.5rem 1rem;
	border-radius: 100px;
	margin-bottom: 1rem;
}

.section-label.on-dark {
	background: rgba(230, 154, 45, 0.15);
}
</style>
