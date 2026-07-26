<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Number of columns at full width, or 'auto' for auto-fit */
		columns: 2 | 3 | 4 | 'auto';
		/** Gap between items */
		gap?: 'sm' | 'md' | 'lg';
		/** Maximum width of the grid container */
		maxWidth?: string;
		/** Minimum item width for auto columns */
		minItemWidth?: string;
		/** Bottom margin */
		marginBottom?: 'none' | 'md' | 'lg';
		/** Additional CSS class for styling hooks */
		class?: string;
		/** Grid content */
		children: Snippet;
	}

	let {
		columns,
		gap = 'md',
		maxWidth,
		minItemWidth = '280px',
		marginBottom = 'none',
		class: className = '',
		children
	}: Props = $props();

	const gapValues = {
		sm: '1rem',
		md: '1.5rem',
		lg: '2rem'
	};

	const marginValues = {
		none: '0',
		md: '3rem',
		lg: '4rem'
	};
</script>

<div
	class="responsive-grid cols-{columns} gap-{gap} mb-{marginBottom} {className}"
	style:--grid-gap={gapValues[gap]}
	style:--grid-max-width={maxWidth}
	style:--grid-min-item={minItemWidth}
	style:--grid-margin-bottom={marginValues[marginBottom]}
>
	{@render children()}
</div>

<style>
	.responsive-grid {
		display: grid;
		grid-template-columns: 1fr;
		gap: var(--grid-gap);
		max-width: var(--grid-max-width, none);
		margin: 0 auto var(--grid-margin-bottom, 0);
	}

	/* 2 columns: 1 → 2 */

	.cols-2 {
		@media (--tablet-up) {
			grid-template-columns: repeat(2, 1fr);
		}
	}

	/* 3 columns: 1 → 2 → 3 */

	.cols-3 {
		@media (--mobile-up) {
			grid-template-columns: repeat(2, 1fr);
		}

		@media (width >= 900px) {
			grid-template-columns: repeat(3, 1fr);
		}
	}

	/* 4 columns: 1 → 2 → 4 */

	.cols-4 {
		@media (--mobile-up) {
			grid-template-columns: repeat(2, 1fr);
		}

		@media (--desktop-up) {
			grid-template-columns: repeat(4, 1fr);
		}
	}

	/* Auto columns: responsive auto-fit */

	.cols-auto {
		@media (--tablet-up) {
			grid-template-columns: repeat(auto-fit, minmax(min(100%, var(--grid-min-item, 280px)), 1fr));
		}
	}
</style>
