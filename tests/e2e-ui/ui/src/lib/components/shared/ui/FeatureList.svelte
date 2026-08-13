<script lang="ts">
	type Item = string | { icon?: string; text: string };

	interface Props {
		/** List items - strings or objects with icon and text */
		items: Item[];
		/** Color theme */
		theme?: 'light' | 'dark';
		/** Indicator shown at end of each item */
		indicator?: 'check' | 'x' | 'none';
		/** Additional CSS class */
		class?: string;
	}

	let {
		items,
		theme = 'light',
		indicator = 'check',
		class: className = ''
	}: Props = $props();

	function getText(item: Item): string {
		return typeof item === 'string' ? item : item.text;
	}

	function getIcon(item: Item): string | undefined {
		return typeof item === 'string' ? undefined : item.icon;
	}
</script>

<ul class="feature-list theme-{theme} {className}">
	{#each items as item, i (i)}
		<li>
			{#if getIcon(item)}
				<span class="item-icon">{getIcon(item)}</span>
			{/if}
			<span class="item-text">{getText(item)}</span>
			{#if indicator === 'check'}
				<span class="item-indicator check">✓</span>
			{:else if indicator === 'x'}
				<span class="item-indicator x">✕</span>
			{/if}
		</li>
	{/each}
</ul>

<style>
.feature-list {
	list-style: none;
	padding: 0;
	margin: 0;
}

.feature-list li {
	display: flex;
	align-items: center;
	gap: 0.75rem;
	padding: 0.75rem 0;
}

.feature-list li:last-child {
	border-bottom: none;
}

.item-icon {
	font-size: 1.25rem;
	flex-shrink: 0;
}

.item-text {
	flex: 1;
}

.item-indicator {
	font-weight: 700;
	font-size: 1.125rem;
	flex-shrink: 0;
}

/* Light theme */
.theme-light {
	& li {
		border-bottom: 1px solid var(--hops-border);
	}

	& .item-text {
		color: var(--hops-text-secondary);
	}

	& .item-indicator.check {
		color: var(--hops-success);
	}

	& .item-indicator.x {
		color: #dc2626;
	}
}

/* Dark theme */
.theme-dark {
	& li {
		border-bottom: 1px solid rgba(255, 255, 255, 0.08);
	}

	& .item-text {
		color: rgba(255, 255, 255, 0.8);
	}

	& .item-indicator.check {
		color: var(--hops-orange);
	}

	& .item-indicator.x {
		color: #f87171;
	}
}
</style>
