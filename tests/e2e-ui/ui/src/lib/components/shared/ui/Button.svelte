<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes, HTMLAnchorAttributes } from 'svelte/elements';

	interface Props {
		/** Button variant */
		variant?: 'primary' | 'secondary' | 'outlined';
		/** Button size */
		size?: 'md' | 'lg';
		/** Light mode for outlined on dark backgrounds */
		light?: boolean;
		/** Link href - renders as <a> when provided */
		href?: string;
		/** Button type (when not a link) */
		type?: 'button' | 'submit' | 'reset';
		/** Disabled state */
		disabled?: boolean;
		/** Click handler */
		onclick?: (e: MouseEvent) => void;
		/** Button content */
		children: Snippet;
	}

	let {
		variant = 'primary',
		size = 'md',
		light = false,
		href,
		type = 'button',
		disabled = false,
		onclick,
		children
	}: Props = $props();

	const isLink = !!href;
</script>

{#if isLink}
	<a
		{href}
		class="button button-{variant} button-{size}"
		class:light
		{onclick}
	>
		{@render children()}
	</a>
{:else}
	<button
		{type}
		{disabled}
		class="button button-{variant} button-{size}"
		class:light
		{onclick}
	>
		{@render children()}
	</button>
{/if}

<style>
.button {
	display: inline-flex;
	align-items: center;
	justify-content: center;
	gap: 0.5rem;
	font-family: var(--font-body);
	font-weight: 600;
	font-size: 1rem;
	padding: 0.875rem 1.75rem;
	border-radius: 8px;
	border: none;
	cursor: pointer;
	text-decoration: none;
	transition: all 0.3s var(--ease-out-expo);
	position: relative;
	overflow: hidden;

	&:disabled {
		opacity: 0.6;
		cursor: not-allowed;
		transform: none !important;
	}
}

/* Sizes */
.button-lg {
	padding: 1.125rem 2.25rem;
	font-size: 1.125rem;

	@media (--tablet) {
		padding: 1rem 1.5rem;
		font-size: 1rem;
	}

	@media (--mobile) {
		padding: 0.875rem 1.25rem;
		font-size: 0.95rem;
		width: 100%;
	}
}

/* Primary variant */
.button-primary {
	background: var(--hops-orange);
	color: var(--hops-text-inverse);
	box-shadow: var(--shadow-orange);

	&:hover:not(:disabled) {
		background: var(--hops-orange-light);
		transform: translateY(-3px);
		box-shadow: 0 12px 40px rgba(230, 154, 45, 0.45);
	}

	&:active:not(:disabled) {
		transform: translateY(-1px);
	}

	/* Motion streak on hover */
	&::after {
		content: '';
		position: absolute;
		top: 50%;
		left: -100%;
		width: 60px;
		height: 3px;
		background: linear-gradient(90deg, transparent, rgba(255,255,255,0.8), transparent);
		transform: translateY(-50%) skewX(-20deg);
		transition: left 0.5s var(--ease-out-expo);
	}

	&:hover:not(:disabled)::after {
		left: 120%;
	}
}

/* Secondary variant */
.button-secondary {
	background: var(--hops-navy);
	color: var(--hops-text-inverse);

	&:hover:not(:disabled) {
		background: var(--hops-navy-light);
		transform: translateY(-3px);
	}
}

/* Outlined variant */
.button-outlined {
	background: transparent;
	border: 2px solid var(--hops-navy);
	color: var(--hops-navy);

	&:hover:not(:disabled) {
		background: var(--hops-navy);
		color: var(--hops-text-inverse);
		transform: translateY(-3px);
	}

	/* Light mode for dark backgrounds */
	&.light {
		border-color: rgba(255,255,255,0.5);
		color: var(--hops-text-inverse);

		&:hover:not(:disabled) {
			background: rgba(255,255,255,0.15);
			border-color: rgba(255,255,255,0.8);
		}
	}
}

/* Touch target for mobile */
@media (--mobile) {
	.button {
		min-height: 48px;
	}
}
</style>
