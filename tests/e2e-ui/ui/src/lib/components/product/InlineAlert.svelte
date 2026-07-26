<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Short uppercase label (e.g. Mutation, SSR GraphQL) */
		label?: string;
		/** Visual tone */
		tone?: 'danger' | 'info';
		children: Snippet;
	}

	let { label, tone = 'danger', children }: Props = $props();
</script>

<div class="inline-alert tone-{tone}" role="alert">
	{#if label}
		<span class="label">{label}</span>
	{/if}
	<div class="body">
		{@render children()}
	</div>
</div>

<style>
	.inline-alert {
		display: flex;
		gap: 0.75rem;
		align-items: flex-start;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		border-radius: var(--wf-radius, 6px);
		font-size: 0.92rem;
		line-height: 1.45;
	}

	.tone-danger {
		background: rgba(179, 58, 58, 0.08);
		border: 1px solid rgba(179, 58, 58, 0.22);
		color: var(--wf-danger, #b33a3a);
	}

	.tone-info {
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		border: 1px solid var(--wf-line, #e2e0d9);
		color: var(--wf-ink-soft, #5c5c56);
	}

	.label {
		font-weight: 700;
		font-size: 0.7rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		opacity: 0.8;
		padding-top: 0.15rem;
		flex-shrink: 0;
	}

	.body {
		min-width: 0;
		flex: 1;
	}
</style>
