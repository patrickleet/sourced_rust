<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Show pulsing status dot */
		dot?: boolean;
		/** Badge variant */
		variant?: 'navy' | 'light' | 'outline';
		/** Add border to badge */
		bordered?: boolean;
		/** Custom icon snippet (alternative to dot) */
		icon?: Snippet;
		/** Badge content */
		children: Snippet;
	}

	let { dot = false, variant = 'navy', bordered = false, icon, children }: Props = $props();
</script>

<span class="badge badge-{variant}" class:bordered>
	{#if dot}
		<span class="badge-dot"></span>
	{:else if icon}
		<span class="badge-icon">{@render icon()}</span>
	{/if}
	{@render children()}
</span>

<style>
	.badge {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.5rem 1rem;
		border-radius: 100px;
		font-size: 0.875rem;
		font-weight: 600;
		letter-spacing: 0.02em;
		&-navy {
			background: var(--hops-navy);
			color: var(--hops-text-inverse);
		}
		&-light {
			background: rgba(230, 154, 45, 0.15);
			color: var(--hops-orange);
		}
		&-outline {
			background: transparent;
			border: 1px solid var(--hops-border);
			color: var(--hops-text-primary);
		}
		&.bordered {
			border: 1px solid currentColor;
			border-color: rgba(230, 154, 45, 0.3);
		}
		&-dot {
			width: 8px;
			height: 8px;
			background: var(--hops-orange);
			border-radius: 50%;
			animation: pulse 2s ease-in-out infinite;
		}
		&-icon {
			display: inline-flex;
			align-items: center;
			justify-content: center;
			font-size: 0.875em;
		}
	}

	@keyframes pulse {
		0%, 100% {
			opacity: 1;
			transform: scale(1);
		}
		50% {
			opacity: 0.6;
			transform: scale(0.9);
		}
	}
</style>
