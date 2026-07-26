<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Row label */
		label: string;
		/** Use monospace font for value */
		mono?: boolean;
		/** Row value - can be string or snippet for custom content */
		value?: string;
		/** Custom content slot */
		children?: Snippet;
	}

	let { label, mono = false, value, children }: Props = $props();
</script>

<div class="data-row">
	<dt class="label">{label}</dt>
	<dd class="value" class:mono>
		{#if children}
			{@render children()}
		{:else}
			{value || '—'}
		{/if}
	</dd>
</div>

<style>
	.data-row {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 1rem 1.25rem;
		border-bottom: 1px solid var(--hops-border);
		gap: 1rem;
		&:last-child {
			border-bottom: none;
		}
	}

	.label {
		font-size: 0.875rem;
		color: var(--hops-text-muted);
		flex-shrink: 0;
	}

	.value {
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--hops-text-primary);
		text-align: right;
		word-break: break-word;
		margin: 0;
		&.mono {
			font-family: var(--font-mono);
			font-size: 0.8rem;
			font-weight: 400;
		}
	}
</style>
