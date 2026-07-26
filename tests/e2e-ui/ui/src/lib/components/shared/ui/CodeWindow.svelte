<script lang="ts">
	interface Props {
		/** Filename or label shown in header */
		filename?: string;
		/** Code content to display */
		code: string;
		/** Size variant */
		size?: 'sm' | 'md' | 'lg';
		/** Enable hover lift effect */
		hover?: boolean;
		/** Enable 3D perspective tilt on desktop */
		perspective?: boolean;
	}

	let {
		filename = '',
		code,
		size = 'md',
		hover = false,
		perspective = false
	}: Props = $props();
</script>

<div
	class="code-window size-{size}"
	class:has-hover={hover}
	class:has-perspective={perspective}
>
	<div class="code-header">
		<div class="code-dots">
			<span></span>
			<span></span>
			<span></span>
		</div>
		{#if filename}
			<span class="code-filename">{filename}</span>
		{/if}
	</div>
	<pre class="code-content"><code>{code}</code></pre>
</div>

<style>
	.code-window {
		background: var(--hops-navy-deep);
		border-radius: 12px;
		overflow: hidden;
		box-shadow: 0 10px 40px rgba(26, 39, 68, 0.2), 0 0 0 1px rgba(26, 39, 68, 0.1);
		transition: transform 0.4s var(--ease-out-expo), box-shadow 0.4s var(--ease-out-expo);

		&.size-lg {
			border-radius: 16px;
			box-shadow: 0 20px 60px rgba(26, 39, 68, 0.25), 0 0 0 1px rgba(26, 39, 68, 0.1);
		}

		&.has-hover:hover {
			transform: translateY(-4px);
			box-shadow: 0 20px 60px rgba(26, 39, 68, 0.3), 0 0 0 1px rgba(26, 39, 68, 0.1);
		}

		&.has-perspective {
			@media (--desktop-up) {
				transform: perspective(1000px) rotateY(-2deg) rotateX(2deg);

				&:hover {
					transform: perspective(1000px) rotateY(0deg) rotateX(0deg);
				}
			}
		}
	}

	.code-header {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.75rem 1rem;
		background: rgba(0, 0, 0, 0.3);
		border-bottom: 1px solid rgba(255, 255, 255, 0.05);

		.size-lg & {
			gap: 1rem;
			padding: 1rem 1.25rem;
		}
	}

	.code-dots {
		display: flex;
		gap: 6px;

		& span {
			width: 10px;
			height: 10px;
			border-radius: 50%;
			background: var(--hops-orange);
			opacity: 0.8;

			&:nth-child(2) {
				opacity: 0.5;
			}

			&:nth-child(3) {
				opacity: 0.3;
			}
		}

		.size-lg & span {
			width: 12px;
			height: 12px;
		}

		.size-lg & {
			gap: 8px;
		}
	}

	.code-filename {
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: rgba(255, 255, 255, 0.5);

		.size-lg & {
			font-size: 0.8rem;
		}
	}

	.code-content {
		margin: 0;
		padding: 1rem;
		overflow-x: auto;
		background: transparent;

		& code {
			display: block;
			font-family: var(--font-mono);
			font-size: 0.8rem;
			line-height: 1.5;
			color: #e2e8f0;
			background: transparent;
			white-space: pre;
			text-align: left;
		}

		.size-sm & {
			padding: 0.75rem;

			& code {
				font-size: 0.75rem;
			}
		}

		.size-lg & {
			padding: 1rem;

			& code {
				font-size: 0.75rem;
				line-height: 1.7;

				@media (--tablet-up) {
					font-size: 0.85rem;
					padding: 0.5rem;
				}
			}

			@media (--tablet-up) {
				padding: 1.5rem;
			}
		}
	}
</style>
