<script lang="ts">
	interface Props {
		/** Code content */
		code: string;
		/** Optional title */
		title?: string;
		/** Make it collapsible */
		collapsible?: boolean;
		/** Max height before scroll */
		maxHeight?: string;
	}

	let { code, title, collapsible = false, maxHeight = '300px' }: Props = $props();

	let copied = $state(false);

	async function copyCode() {
		try {
			await navigator.clipboard.writeText(code);
			copied = true;
			setTimeout(() => copied = false, 2000);
		} catch (err) {
			console.error('Failed to copy:', err);
		}
	}
</script>

{#if collapsible}
	<details class="code-block collapsible">
		<summary class="code-header">
			<span class="code-title">{title || 'Code'}</span>
			<svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<polyline points="6 9 12 15 18 9"></polyline>
			</svg>
		</summary>
		<div class="code-content">
			<button class="copy-btn" onclick={copyCode} class:copied>
				{#if copied}
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
						<polyline points="20 6 9 17 4 12"></polyline>
					</svg>
					<span>Copied</span>
				{:else}
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
						<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
					</svg>
					<span>Copy</span>
				{/if}
			</button>
			<pre style="max-height: {maxHeight}">{code}</pre>
		</div>
	</details>
{:else}
	<div class="code-block">
		{#if title}
			<div class="code-header static">
				<span class="code-title">{title}</span>
				<button class="copy-btn" onclick={copyCode} class:copied>
					{#if copied}
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
							<polyline points="20 6 9 17 4 12"></polyline>
						</svg>
						<span>Copied</span>
					{:else}
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
							<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
						</svg>
						<span>Copy</span>
					{/if}
				</button>
			</div>
		{/if}
		<pre style="max-height: {maxHeight}">{code}</pre>
	</div>
{/if}

<style>
	.code-block {
		border: 1px solid var(--hops-border);
		border-radius: 8px;
		overflow: hidden;
		background: var(--hops-bg-white);
	}

	.code-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 0.75rem 1rem;
		background: var(--hops-bg-light);
		border-bottom: 1px solid var(--hops-border);
		&.static {
			cursor: default;
		}
	}

	.collapsible > .code-header {
		cursor: pointer;
		user-select: none;
	}

	.collapsible > .code-header:hover {
		background: var(--hops-bg-cream);
	}

	.code-title {
		font-family: var(--font-display);
		font-size: 0.8rem;
		font-weight: 600;
		color: var(--hops-text-secondary);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.chevron {
		color: var(--hops-text-muted);
		transition: transform 0.2s ease;
	}

	.collapsible[open] .chevron {
		transform: rotate(180deg);
	}

	.collapsible::-webkit-details-marker {
		display: none;
	}

	.code-content {
		position: relative;
	}

	.copy-btn {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		padding: 0.35rem 0.6rem;
		background: var(--hops-bg-white);
		border: 1px solid var(--hops-border);
		border-radius: 6px;
		cursor: pointer;
		color: var(--hops-text-muted);
		font-size: 0.7rem;
		font-weight: 500;
		transition: all 0.15s ease;
		&:hover {
			border-color: var(--hops-navy);
			color: var(--hops-navy);
		}
		&.copied {
			background: var(--hops-success);
			border-color: var(--hops-success);
			color: white;
		}
	}

	.collapsible .copy-btn {
		position: absolute;
		top: 0.75rem;
		right: 0.75rem;
		z-index: 1;
	}

	pre {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		color: #a0aec0;
		background: var(--hops-navy-deep);
		padding: 1rem;
		margin: 0;
		overflow: auto;
		white-space: pre-wrap;
		word-break: break-word;
		line-height: 1.6;
	}
</style>
