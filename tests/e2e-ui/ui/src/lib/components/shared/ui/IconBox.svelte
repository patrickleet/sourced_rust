<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		/** Size variant */
		size?: 'sm' | 'md' | 'lg';
		/** Style variant */
		variant?: 'gradient' | 'navy' | 'light';
		/** Icon content */
		children: Snippet;
	}

	let { size = 'md', variant = 'gradient', children }: Props = $props();
</script>

<div class="icon-box icon-box-{size} icon-box-{variant}">
	{@render children()}
</div>

<style>
.icon-box {
	display: inline-flex;
	align-items: center;
	justify-content: center;
	border-radius: 16px;
	color: var(--hops-text-inverse);
}

/* Sizes */
.icon-box-sm {
	width: 48px;
	height: 48px;
	border-radius: 12px;
}

.icon-box-md {
	width: 64px;
	height: 64px;
}

.icon-box-lg {
	width: 80px;
	height: 80px;
	border-radius: 20px;
}

/* Variants */
.icon-box-gradient {
	background: linear-gradient(135deg, var(--hops-orange) 0%, var(--hops-orange-dark) 100%);
	box-shadow: var(--shadow-orange);
}

.icon-box-navy {
	background: var(--hops-navy);
	box-shadow: var(--shadow-md);
}

.icon-box-light {
	background: var(--hops-bg-light);
	color: var(--hops-orange);
	box-shadow: var(--shadow-sm);
}
</style>
