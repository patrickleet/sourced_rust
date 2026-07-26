<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Color theme */
		theme?: 'light' | 'dark' | 'glass';
		/** Padding size */
		padding?: 'sm' | 'md' | 'lg' | 'none';
		/** Enable hover lift and border accent */
		hover?: boolean;
		/** Animation delay in seconds */
		delay?: number;
		/** Optional: show streak animation on hover (dark theme) */
		streak?: boolean;
		/** Featured/highlighted state (orange border + accent bar) */
		featured?: boolean;
		/** Optional header title (creates a navy header bar) */
		headerTitle?: string;
		/** Card content */
		children: Snippet;
	}

	let {
		theme = 'light',
		padding = 'md',
		hover = true,
		delay = 0,
		streak = false,
		featured = false,
		headerTitle,
		children
	}: Props = $props();
</script>

<div
	class="card theme-{theme}"
	class:padding-sm={padding === 'sm' && !headerTitle}
	class:padding-md={padding === 'md' && !headerTitle}
	class:padding-lg={padding === 'lg' && !headerTitle}
	class:has-hover={hover}
	class:has-streak={streak}
	class:is-featured={featured}
	class:has-header={!!headerTitle}
	style:--delay="{delay}s"
>
	{#if headerTitle}
		<header class="card-header">
			<p class="card-header-title">{headerTitle}</p>
		</header>
		<div class="card-body padding-{padding}">
			{@render children()}
		</div>
	{:else}
		{@render children()}
	{/if}
	{#if streak}
		<div class="card-streak"></div>
	{/if}
	{#if featured}
		<div class="card-accent"></div>
	{/if}
</div>

<style>
	.card {
		position: relative;
		border-radius: 20px;
		overflow: hidden;
		animation: fadeInUp 0.5s var(--ease-out-expo) both;
		animation-delay: var(--delay);
		transition: all 0.4s var(--ease-out-expo);
		&.has-header {
			border-radius: 16px;
		}
		&-header {
			background: var(--hops-navy);
			padding: 1rem 1.5rem;
			&-title {
				font-family: var(--font-display);
				font-size: 1rem;
				font-weight: 700;
				color: var(--hops-text-inverse);
				margin: 0;
				text-transform: uppercase;
				letter-spacing: 0.05em;
			}
		}
		&-accent {
			position: absolute;
			top: 0;
			left: 0;
			right: 0;
			height: 4px;
			background: linear-gradient(90deg, var(--hops-orange), var(--hops-orange-light));
		}
		&-streak {
			position: absolute;
			top: 0;
			left: -100px;
			width: 100px;
			height: 100%;
			background: linear-gradient(90deg, transparent, rgba(230, 154, 45, 0.1), transparent);
			transform: skewX(-15deg) translateX(-100px);
			opacity: 0;
			transition: all 0.6s ease;
			pointer-events: none;
		}
	}

	/* Padding variants */

	.padding-sm {
		padding: 1.25rem;

		@media (--tablet-up) {
			padding: 1.5rem;
		}
	}

	.padding-md {
		padding: 1.5rem;

		@media (--tablet-up) {
			padding: 2rem;
		}
	}

	.padding-lg {
		padding: 2rem;

		@media (--tablet-up) {
			padding: 2.5rem;
		}
	}

	.padding-none {
		padding: 0;
	}

	/* Header styles */

	/* Light theme */

	.theme-light {
		background: var(--hops-bg-white);
		border: 2px solid var(--hops-border);

		&.has-hover:hover {
			border-color: var(--hops-orange);
			transform: translateY(-4px);
			box-shadow: var(--shadow-lg);
		}

		&.is-featured {
			border-color: var(--hops-orange);
			background: linear-gradient(180deg, rgba(230, 154, 45, 0.05) 0%, var(--hops-bg-white) 100%);
		}

		&.has-header {
			border: 1px solid var(--hops-border);
			box-shadow: var(--shadow-sm);
		}
	}

	/* Dark theme */

	.theme-dark {
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);

		&.has-hover:hover {
			transform: translateY(-8px);
			border-color: rgba(230, 154, 45, 0.3);
			background: rgba(255, 255, 255, 0.08);
		}

		&.is-featured {
			border-color: var(--hops-orange);
			background: rgba(230, 154, 45, 0.08);

			&.has-hover:hover {
				background: rgba(230, 154, 45, 0.12);
			}
		}
	}

	/* Glass theme (for nested cards in dark sections) */

	.theme-glass {
		background: rgba(0, 0, 0, 0.25);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 16px;

		&.has-hover:hover {
			background: rgba(230, 154, 45, 0.1);
			border-color: rgba(230, 154, 45, 0.25);
			transform: translateY(-4px);
		}
	}

	/* Featured accent bar */

	/* Streak animation (dark theme only) */

	.has-streak:hover .card-streak {
		opacity: 1;
		transform: skewX(-15deg) translateX(400px);
	}
</style>
