<script lang="ts">
	import { SectionLabel } from '$lib/components/shared/ui';
	import type { Snippet } from 'svelte';

	interface Props {
		/** Text for the section label */
		label: string;
		/** Optional subtitle text */
		subtitle?: string;
		/** Theme for dark backgrounds */
		theme?: 'light' | 'dark';
		/** Text alignment */
		align?: 'left' | 'center';
		/** Title content as snippet (allows <br />, <span class="title-accent">, etc.) */
		title: Snippet;
	}

	let {
		label,
		subtitle,
		theme = 'light',
		align = 'center',
		title
	}: Props = $props();
</script>

<div class="section-header theme-{theme} align-{align}">
	<SectionLabel onDark={theme === 'dark'}>{label}</SectionLabel>
	<h2 class="section-title">
		{@render title()}
	</h2>
	{#if subtitle}
		<p class="section-subtitle">{subtitle}</p>
	{/if}
</div>

<style>
	.section-header {
		margin-bottom: 3rem;
	}

	.align-center {
		text-align: center;
		.section-subtitle {
			margin: 0 auto;
		}
	}

	.align-left {
		text-align: left;
	}

	.section-title {
		font-family: var(--font-display);
		font-size: clamp(1.75rem, 5vw, 3rem);
		font-weight: 800;
		line-height: 1.15;
		margin-bottom: 0.75rem;
		letter-spacing: -0.02em;
	}

	.section-subtitle {
		font-size: 1.125rem;
		line-height: 1.6;
		max-width: 600px;
		margin: 0;
	}

	/* Light theme */

	.theme-light {
		& .section-title {
			color: var(--hops-navy);
		}

		& .section-subtitle {
			color: var(--hops-text-secondary);
		}
	}

	/* Dark theme */

	.theme-dark {
		& .section-title {
			color: var(--hops-text-inverse);
		}

		& .section-subtitle {
			color: rgba(255, 255, 255, 0.7);
		}
	}
</style>
