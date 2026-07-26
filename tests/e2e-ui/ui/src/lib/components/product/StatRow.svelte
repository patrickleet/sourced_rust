<script lang="ts">
	export type Stat = { value: number | string; label: string };

	interface Props {
		stats: Stat[];
	}

	let { stats }: Props = $props();
</script>

<div class="stats">
	{#each stats as s}
		<div class="stat">
			<span class="n">{s.value}</span>
			<span class="l">{s.label}</span>
		</div>
	{/each}
</div>

<style>
	.stats {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1.25rem;
		flex-wrap: wrap;
	}

	.stat {
		display: flex;
		align-items: baseline;
		gap: 0.35rem;
		padding: 0.35rem 0.7rem;
		border-radius: 999px;
		border: 1px solid var(--wf-line, #e2e0d9);
	}

	.n {
		font-weight: 700;
		font-size: 0.95rem;
		font-variant-numeric: tabular-nums;
		color: var(--wf-ink, #1c1c1a);
	}

	.l {
		font-size: 0.72rem;
		font-weight: 500;
		letter-spacing: 0.03em;
		text-transform: uppercase;
		color: var(--wf-ink-soft, #5c5c56);
	}
</style>
