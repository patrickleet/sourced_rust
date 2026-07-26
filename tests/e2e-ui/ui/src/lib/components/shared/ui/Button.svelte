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

	const isLink = $derived(!!href);
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
		font-family: var(--wf-sans, var(--font-body));
		font-weight: 600;
		font-size: 0.95rem;
		padding: 0.7rem 1.25rem;
		border-radius: var(--wf-radius, 6px);
		border: none;
		cursor: pointer;
		text-decoration: none;
		transition: background 0.15s ease, border-color 0.15s ease, color 0.15s ease;
		position: relative;

		&:disabled {
			opacity: 0.55;
			cursor: not-allowed;
		}
		&-lg {
			padding: 0.85rem 1.5rem;
			font-size: 1rem;

			@media (--tablet) {
				padding: 0.75rem 1.25rem;
				font-size: 0.95rem;
			}

			@media (--mobile) {
				padding: 0.75rem 1.1rem;
				font-size: 0.95rem;
				width: 100%;
			}
		}
		&-primary {
			background: var(--wf-accent, var(--hops-orange));
			color: #fff;

			&:hover:not(:disabled) {
				background: var(--hops-orange-light, #5a7a9e);
			}
		}
		&-secondary {
			background: var(--wf-ink, var(--hops-navy));
			color: var(--wf-bg, #f6f5f2);

			&:hover:not(:disabled) {
				background: var(--hops-navy-light, #2a2a28);
			}
		}
		&-outlined {
			background: transparent;
			border: 1px solid var(--wf-line-strong, var(--hops-navy));
			color: var(--wf-ink, var(--hops-navy));

			&:hover:not(:disabled) {
				background: var(--wf-ink, var(--hops-navy));
				border-color: var(--wf-ink, var(--hops-navy));
				color: var(--wf-bg, #f6f5f2);
			}

			/* Light mode for dark backgrounds */
			&.light {
				border-color: rgba(255, 255, 255, 0.45);
				color: var(--hops-text-inverse, #f6f5f2);

				&:hover:not(:disabled) {
					background: rgba(255, 255, 255, 0.12);
					border-color: rgba(255, 255, 255, 0.75);
					color: var(--hops-text-inverse, #f6f5f2);
				}
			}
		}
		@media (--mobile) {
			min-height: 44px;

		}
	}

	/* Sizes */

	/* Primary variant */

	/* Secondary variant */

	/* Outlined variant */

	/* Touch target for mobile */
</style>
