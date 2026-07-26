<script lang="ts">
	interface Tab {
		icon?: string;
		label: string;
	}

	interface Props {
		/** Tab items */
		tabs: Tab[];
		/** Currently active tab index */
		activeIndex?: number;
		/** Color theme */
		theme?: 'light' | 'dark';
		/** Callback when tab changes */
		onChange?: (index: number) => void;
	}

	let {
		tabs,
		activeIndex = $bindable(0),
		theme = 'dark',
		onChange
	}: Props = $props();

	function selectTab(index: number) {
		activeIndex = index;
		onChange?.(index);
	}
</script>

<div class="tab-nav theme-{theme}">
	{#each tabs as tab, i (i)}
		<button
			class="tab"
			class:active={activeIndex === i}
			onclick={() => selectTab(i)}
		>
			{#if tab.icon}
				<span class="tab-icon">{tab.icon}</span>
			{/if}
			<span class="tab-label">{tab.label}</span>
		</button>
	{/each}
</div>

<style>
	.tab-nav {
		display: flex;
		justify-content: center;
		gap: 0.5rem;
		margin-bottom: 2.5rem;
		flex-wrap: wrap;
	}

	.tab {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.625rem 1rem;
		border-radius: 100px;
		font-family: var(--font-body);
		font-size: 0.8rem;
		font-weight: 600;
		cursor: pointer;
		transition: all 0.3s var(--ease-out-expo);
		&-icon {
			font-size: 1.25rem;
			line-height: 1;
		}
		&-label {
			display: none;
			white-space: nowrap;

			@media (--tablet-up) {
				display: inline;
			}
		}
	}

	@media (--tablet-up) {
		.tab {
			padding: 0.75rem 1.25rem;
			font-size: 0.9rem;

		}
		.tab-icon {
			font-size: 1.1rem;

		}
	}

	/* Dark theme */

	.theme-dark {
		& .tab {
			background: rgba(255, 255, 255, 0.05);
			border: 1px solid rgba(255, 255, 255, 0.1);
			color: rgba(255, 255, 255, 0.7);

			&:hover {
				background: rgba(255, 255, 255, 0.1);
				border-color: rgba(255, 255, 255, 0.2);
				color: var(--hops-text-inverse);
			}

			&.active {
				background: var(--hops-orange);
				border-color: var(--hops-orange);
				color: var(--hops-text-inverse);
				box-shadow: 0 4px 20px rgba(230, 154, 45, 0.4);
			}
		}
	}

	/* Light theme */

	.theme-light {
		& .tab {
			background: var(--hops-bg-white);
			border: 1px solid var(--hops-border);
			color: var(--hops-text-secondary);

			&:hover {
				background: var(--hops-bg-cream);
				border-color: var(--hops-border-strong);
				color: var(--hops-navy);
			}

			&.active {
				background: var(--hops-orange);
				border-color: var(--hops-orange);
				color: var(--hops-text-inverse);
				box-shadow: 0 4px 20px rgba(230, 154, 45, 0.3);
			}
		}
	}
</style>
