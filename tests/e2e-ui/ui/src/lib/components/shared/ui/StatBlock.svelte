<script lang="ts">
	interface Props {
		/** The main value to display */
		value: string;
		/** Optional unit suffix (e.g., "%", "months") */
		unit?: string;
		/** Description label */
		label: string;
		/** Color theme */
		theme?: 'light' | 'dark';
		/** Size variant */
		size?: 'sm' | 'md' | 'lg';
		/** Animation delay in seconds */
		delay?: number;
	}

	let {
		value,
		unit = '',
		label,
		theme = 'light',
		size = 'md',
		delay = 0
	}: Props = $props();
</script>

<div
	class="stat-block theme-{theme} size-{size}"
	style:--delay="{delay}s"
>
	<div class="stat-value">
		{value}{#if unit}<span class="stat-unit">{unit}</span>{/if}
	</div>
	<div class="stat-label">{label}</div>
</div>

<style>
	.stat-block {
		text-align: center;
		animation: fadeInUp 0.5s var(--ease-out-expo) both;
		animation-delay: var(--delay);
	}

	/* Size variants */

	.size-sm .stat-value {
		font-size: 2rem;

		@media (--tablet-up) {
			font-size: 2.5rem;
		}
	}

	.size-md .stat-value {
		font-size: 2.5rem;

		@media (--tablet-up) {
			font-size: 3rem;
		}
	}

	.size-lg .stat-value {
		font-size: 3rem;

		@media (--tablet-up) {
			font-size: 3.5rem;
		}
	}

	.stat-value {
		font-family: var(--font-display);
		font-weight: 900;
		line-height: 1;
		margin-bottom: 0.5rem;
		letter-spacing: -0.02em;
	}

	.stat-unit {
		font-weight: 800;
		margin-left: 0.125rem;
	}

	.size-sm .stat-unit {
		font-size: 1rem;

		@media (--tablet-up) {
			font-size: 1.25rem;
		}
	}

	.size-md .stat-unit {
		font-size: 1.25rem;

		@media (--tablet-up) {
			font-size: 1.5rem;
		}
	}

	.size-lg .stat-unit {
		font-size: 1.5rem;

		@media (--tablet-up) {
			font-size: 1.75rem;
		}
	}

	.stat-label {
		font-size: 0.875rem;
		line-height: 1.4;
	}

	/* Light theme */

	.theme-light {
		& .stat-value {
			color: var(--hops-navy);
		}

		& .stat-unit {
			color: var(--hops-orange);
		}

		& .stat-label {
			color: var(--hops-text-muted);
			text-transform: uppercase;
			letter-spacing: 0.1em;
		}
	}

	/* Dark theme */

	.theme-dark {
		& .stat-value {
			color: var(--hops-orange);
		}

		& .stat-unit {
			color: rgba(255, 255, 255, 0.5);
		}

		& .stat-label {
			color: rgba(255, 255, 255, 0.7);
		}
	}
</style>
