<script lang="ts">
	interface Props {
		/** Status variant */
		status: 'success' | 'warning' | 'error' | 'neutral';
		/** Badge text */
		label: string;
	}

	let { status, label }: Props = $props();
</script>

<span class="status-badge {status}">
	{#if status === 'success'}
		<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
			<polyline points="20 6 9 17 4 12"></polyline>
		</svg>
	{:else if status === 'warning'}
		<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
			<circle cx="12" cy="12" r="10"></circle>
			<line x1="12" y1="8" x2="12" y2="12"></line>
			<line x1="12" y1="16" x2="12.01" y2="16"></line>
		</svg>
	{:else if status === 'error'}
		<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
			<line x1="18" y1="6" x2="6" y2="18"></line>
			<line x1="6" y1="6" x2="18" y2="18"></line>
		</svg>
	{/if}
	<span>{label}</span>
</span>

<style>
.status-badge {
	display: inline-flex;
	align-items: center;
	gap: 0.35rem;
	padding: 0.3rem 0.65rem;
	border-radius: 6px;
	font-size: 0.7rem;
	font-weight: 600;
	text-transform: uppercase;
	letter-spacing: 0.03em;
}

.success {
	background: rgba(56, 161, 105, 0.12);
	color: var(--hops-success);
}

.warning {
	background: rgba(221, 107, 32, 0.12);
	color: var(--hops-warning);
}

.error {
	background: rgba(229, 62, 62, 0.12);
	color: var(--hops-danger);
}

.neutral {
	background: var(--hops-bg-light);
	color: var(--hops-text-muted);
}
</style>
