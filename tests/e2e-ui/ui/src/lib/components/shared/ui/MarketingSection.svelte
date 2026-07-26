<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Section id for anchor links */
		id?: string;
		/** Color scheme - controls background and text colors */
		theme?: 'light-primary' | 'light-secondary' | 'dark' | 'transparent-light' | 'transparent-dark';
		/** Visual shape effect */
		effect?: 'callout-right' | 'callout-left';
		/** Show scroll arrow indicator */
		arrow?: boolean;
		/** Section content */
		children: Snippet;
	}

	let {
		id,
		theme = 'light-primary',
		effect,
		arrow = false,
		children
	}: Props = $props();

	// Arrow is automatically light on dark themes
	const isLightArrow = $derived(
		theme === 'dark' || theme === 'transparent-dark'
	);
</script>

<section
	{id}
	class="marketing-section theme-{theme}"
	class:has-arrow={arrow}
	class:has-effect={effect}
	class:effect-callout-right={effect === 'callout-right'}
	class:effect-callout-left={effect === 'callout-left'}
>
	<div class="container">
		{@render children()}
	</div>

	{#if arrow}
		<div class="scroll-indicator" class:is-light={isLightArrow}>
			<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<path d="M7 10l5 5 5-5" />
			</svg>
		</div>
	{/if}
</section>

<style>
	.marketing-section {
		padding: var(--section-padding) 0;
		position: relative;
	}

	/* Themes - color schemes */

	.theme-light-primary {
		background: var(--hops-bg-white);
		color: var(--hops-text-primary);
	}

	.theme-light-secondary {
		background: var(--hops-bg-light);
		color: var(--hops-text-primary);
	}

	.theme-dark {
		background: var(--hops-navy);
		color: var(--hops-text-inverse);
		:global(.section-title) {
			color: var(--hops-text-inverse);
		}
		:global(.section-subtitle) {
			color: rgba(255, 255, 255, 0.7);
		}
	}

	.theme-transparent-light {
		background: transparent;
		color: var(--hops-text-primary);
	}

	.theme-transparent-dark {
		background: transparent;
		color: var(--hops-text-inverse);
	}

	/* Section with scroll indicator */

	.has-arrow {
		padding-bottom: calc(var(--section-padding) + 4rem);
	}

	.scroll-indicator {
		position: absolute;
		bottom: calc(var(--section-padding) / 2);
		left: 50%;
		transform: translateX(-50%);
		color: var(--hops-orange);

		& svg {
			width: 44px;
			height: 44px;
			animation: bounce 2s ease-in-out infinite;
		}

		&.is-light {
			color: rgba(255, 255, 255, 0.7);
		}
	}

	@keyframes bounce {
		0%, 100% {
			transform: translateY(0);
			opacity: 0.6;
		}
		50% {
			transform: translateY(10px);
			opacity: 1;
		}
	}

	/* Dark theme title overrides */

	/* Effect: Callout diagonal transitions */

	.has-effect {
		--diagonal-height: 4rem;
		position: relative;
		z-index: 1;
		margin-top: calc(-1 * var(--diagonal-height));
		margin-bottom: calc(-1 * var(--diagonal-height));
		padding-top: calc(var(--section-padding) + var(--diagonal-height));
		padding-bottom: calc(var(--section-padding) + var(--diagonal-height));
		&.has-arrow {
			padding-bottom: calc(var(--section-padding) + var(--diagonal-height) + 4rem);
		}
	}

	/* Callout right: grows left to right (top angles down-left, bottom angles down-right) */

	.effect-callout-right {
		clip-path: polygon(
			0 var(--diagonal-height),
			100% 0,
			100% 100%,
			0 calc(100% - var(--diagonal-height))
		);
	}

	/* Callout left: grows right to left (mirrored) */

	.effect-callout-left {
		clip-path: polygon(
			0 0,
			100% var(--diagonal-height),
			100% calc(100% - var(--diagonal-height)),
			0 100%
		);
	}

	/* When effect has arrow, add extra padding */
</style>
