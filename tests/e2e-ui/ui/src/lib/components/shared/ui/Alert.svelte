<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Alert variant */
		variant?: 'warning' | 'success' | 'error' | 'info';
		/** Optional title */
		title?: string;
		/** Alert content */
		children: Snippet;
	}

	let {
		variant = 'info',
		title,
		children
	}: Props = $props();
</script>

<div class="alert variant-{variant}">
	{#if title}
		<h2 class="alert-title">{title}</h2>
	{/if}
	<div class="alert-content">
		{@render children()}
	</div>
</div>

<style>
	.alert {
		border-radius: 12px;
		padding: 1.5rem;
		text-align: center;

		@media (--tablet-up) {
			padding: 2rem;
		}
		&-title {
			font-family: var(--font-display);
			font-size: 1.5rem;
			font-weight: 700;
			margin: 0 0 0.75rem;
		}
		&-content {
			& :global(p) {
				margin: 0 0 1rem;
			}

			& :global(p:last-child) {
				margin-bottom: 0;
			}
		}
	}

	/* Variants */

	.variant-warning {
		background: rgba(221, 107, 32, 0.1);
		border: 1px solid rgba(221, 107, 32, 0.3);

		& .alert-title {
			color: var(--hops-warning);
		}

		& .alert-content {
			color: var(--hops-text-secondary);
		}
	}

	.variant-success {
		background: rgba(76, 175, 80, 0.15);
		border: 1px solid rgba(76, 175, 80, 0.3);

		& .alert-title {
			color: #4caf50;
		}

		& .alert-content {
			color: var(--hops-text-secondary);
		}
	}

	.variant-error {
		background: rgba(244, 67, 54, 0.15);
		border: 1px solid rgba(244, 67, 54, 0.3);

		& .alert-title {
			color: #f44336;
		}

		& .alert-content {
			color: #f44336;
		}
	}

	.variant-info {
		background: rgba(26, 39, 68, 0.05);
		border: 1px solid var(--hops-border);

		& .alert-title {
			color: var(--hops-navy);
		}

		& .alert-content {
			color: var(--hops-text-secondary);
		}
	}

	/* Dark background variants */

	:global(.bg-navy) .variant-warning,
	:global([data-theme="dark"]) .variant-warning {
		background: rgba(221, 107, 32, 0.2);

		& .alert-content {
			color: rgba(255, 255, 255, 0.7);
		}
	}

	:global(.bg-navy) .variant-success,
	:global([data-theme="dark"]) .variant-success {
		& .alert-content {
			color: var(--hops-text-inverse);
		}
	}

	:global(.bg-navy) .variant-error,
	:global([data-theme="dark"]) .variant-error {
		& .alert-content {
			color: #ff6b6b;
		}
	}
</style>
