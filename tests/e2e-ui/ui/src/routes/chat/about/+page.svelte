<script lang="ts">
	import { AppPage, PageHeader } from '$lib/components/product';
</script>

<AppPage>
	<PageHeader kicker="Room · lobby" title="About the lobby">
		This child page intentionally shares the lobby layout so its finite live
		island remains attached while page-owned work is replaced.
	</PageHeader>

	<section class="about-room" aria-labelledby="about-room-heading">
		<h2 id="about-room-heading">One room, one retained stream</h2>
		<p>
			Messages are selected by a layout-owned GraphQL island. Navigating back to
			the conversation reuses the same normalized replica and live subscription.
		</p>
		<a href="/chat">Back to the lobby</a>
	</section>
</AppPage>

<style>
	.about-room {
		max-width: 46rem;
		padding: clamp(1.25rem, 3vw, 2rem);
		border: 1px solid var(--wf-line, #e2e0d9);
		border-radius: var(--df-radius-lg, 10px);
		background: var(--wf-bg-elevated, #fff);
	}

	h2 {
		margin: 0 0 0.65rem;
		font-size: clamp(1.15rem, 2vw, 1.45rem);
	}

	p {
		max-width: 62ch;
		margin: 0 0 1rem;
		color: var(--wf-ink-soft, #5c5c56);
		line-height: 1.6;
	}

	a {
		color: var(--wf-accent, #3d5a80);
		font-weight: 700;
		text-underline-offset: 0.2em;
	}
</style>
