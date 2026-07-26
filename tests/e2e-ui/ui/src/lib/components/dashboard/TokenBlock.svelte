<script lang="ts">
	interface Props {
		/** Token label */
		label: string;
		/** Token type hint */
		hint?: string;
		/** Token value */
		value: string;
	}

	let { label, hint, value }: Props = $props();

	let copied = $state(false);

	async function copyToken() {
		try {
			await navigator.clipboard.writeText(value);
			copied = true;
			setTimeout(() => copied = false, 2000);
		} catch (err) {
			console.error('Failed to copy:', err);
		}
	}
</script>

<div class="token-block">
	<div class="token-header">
		<span class="token-label">{label}</span>
		<div class="token-actions">
			{#if hint}
				<span class="token-hint">{hint}</span>
			{/if}
			<button class="copy-btn" onclick={copyToken} class:copied>
				{#if copied}
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
						<polyline points="20 6 9 17 4 12"></polyline>
					</svg>
				{:else}
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
						<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
					</svg>
				{/if}
			</button>
		</div>
	</div>
	<pre class="token-value">{value}</pre>
</div>

<style>
	.token-block {
		border: 1px solid var(--hops-border);
		border-radius: 8px;
		overflow: hidden;
	}

	.token-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 0.6rem 0.875rem;
		background: var(--hops-bg-light);
		border-bottom: 1px solid var(--hops-border);
	}

	.token-label {
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--hops-text-secondary);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.token-actions {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.token-hint {
		font-size: 0.65rem;
		font-family: var(--font-mono);
		color: var(--hops-text-muted);
		background: var(--hops-bg-white);
		padding: 0.2rem 0.4rem;
		border-radius: 4px;
		border: 1px solid var(--hops-border);
	}

	.copy-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		background: var(--hops-bg-white);
		border: 1px solid var(--hops-border);
		border-radius: 6px;
		cursor: pointer;
		color: var(--hops-text-muted);
		transition: all 0.15s ease;
		&:hover {
			background: var(--hops-bg-white);
			border-color: var(--hops-navy);
			color: var(--hops-navy);
		}
		&.copied {
			background: var(--hops-success);
			border-color: var(--hops-success);
			color: white;
		}
	}

	.token-value {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		color: #a0aec0;
		background: var(--hops-navy-deep);
		padding: 0.875rem;
		margin: 0;
		word-break: break-all;
		white-space: pre-wrap;
		line-height: 1.6;
		max-height: 88px;
		overflow-y: auto;
	}
</style>
