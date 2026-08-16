<script lang="ts">
	/**
	 * Right-hand slide-out: tabbed walkthrough of how a demo is built.
	 * Distributed lens — domain → command → projection → client — not a generic code dump.
	 */
	import type { DemoWalkthrough } from '$lib/walkthrough';
	import { highlightCode } from './highlight';

	interface Props {
		demo: DemoWalkthrough;
		/** Start open (e.g. deep link) */
		defaultOpen?: boolean;
	}

	let { demo, defaultOpen = false }: Props = $props();

	// Intentionally seed once from the prop (deep-link); toggles use `open` only.
	// svelte-ignore state_referenced_locally
	let open = $state(defaultOpen);
	let activeTab = $state(0);

	const tab = $derived(demo.tabs[activeTab] ?? demo.tabs[0]);

	function openPanel() {
		open = true;
		activeTab = 0;
	}

	function closePanel() {
		open = false;
	}

	function onKeydown(e: KeyboardEvent) {
		if (!open) return;
		if (e.key === 'Escape') {
			e.preventDefault();
			closePanel();
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<button type="button" class="hib-fab" onclick={openPanel} aria-expanded={open} aria-controls="hib-drawer-{demo.id}">
	<span class="hib-fab-mark" aria-hidden="true">{'{ }'}</span>
	<span class="hib-fab-text">
		<span class="hib-fab-kicker">How it’s built</span>
		<span class="hib-fab-title">{demo.title}</span>
	</span>
</button>

{#if open}
	<button type="button" class="hib-scrim" aria-label="Close how it’s built" onclick={closePanel}></button>
{/if}

<div
	id="hib-drawer-{demo.id}"
	class="hib-drawer"
	class:open
	role="dialog"
	aria-modal="true"
	aria-labelledby="hib-title-{demo.id}"
	inert={!open}
>
	<header class="hib-head">
		<div class="hib-head-text">
			<span class="hib-kicker">{demo.kicker}</span>
			<h2 id="hib-title-{demo.id}" class="hib-title">How it’s built</h2>
			<p class="hib-summary">{demo.summary}</p>
			<p class="hib-path" aria-hidden="true">
				Browser query → commands → handlers → domain → events → service / host / runner
			</p>
		</div>
		<button type="button" class="hib-close" onclick={closePanel} aria-label="Close">
			<span aria-hidden="true">×</span>
		</button>
	</header>

	<nav class="hib-tabs" aria-label="Walkthrough sections">
		{#each demo.tabs as t, i}
			<button
				type="button"
				class="hib-tab"
				class:active={i === activeTab}
				onclick={() => (activeTab = i)}
				aria-selected={i === activeTab}
				role="tab"
			>
				<span class="hib-tab-n">{String(i + 1).padStart(2, '0')}</span>
				{t.label}
			</button>
		{/each}
	</nav>

	{#if tab}
		<div class="hib-body" role="tabpanel">
			<p class="hib-lede">{tab.lede}</p>
			<blockquote class="hib-principle">
				<span class="hib-principle-label">Principle</span>
				{tab.principle}
			</blockquote>

			{#each tab.samples as sample}
				<figure class="hib-sample">
					<figcaption class="hib-sample-bar">
						<span class="hib-file">{sample.file}</span>
						{#if sample.caption}
							<span class="hib-caption">{sample.caption}</span>
						{/if}
					</figcaption>
					<pre class="hib-code"><code>{@html highlightCode(sample.code)}</code></pre>
				</figure>
			{/each}
		</div>
	{/if}

	<footer class="hib-foot">
		<span class="hib-foot-label">Distributed · e2e-ui</span>
		<a class="hib-foot-link" href="/#demos" onclick={closePanel}>All demos</a>
	</footer>
</div>

<style>
	.hib-fab {
		position: fixed;
		z-index: 40;
		right: max(1rem, env(safe-area-inset-right));
		bottom: max(1.25rem, env(safe-area-inset-bottom));
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.65rem 1rem 0.65rem 0.75rem;
		border: 1px solid var(--wf-line-strong, #cdcabe);
		border-radius: 999px;
		background: var(--wf-ink, #1c1c1a);
		color: var(--wf-bg, #f6f5f2);
		box-shadow:
			0 12px 32px rgba(28, 28, 26, 0.22),
			0 0 0 1px rgba(255, 255, 255, 0.06) inset;
		cursor: pointer;
		font-family: var(--wf-sans, system-ui, sans-serif);
		text-align: left;
		transition:
			transform 0.25s var(--ease, cubic-bezier(0.22, 1, 0.36, 1)),
			box-shadow 0.25s var(--ease, cubic-bezier(0.22, 1, 0.36, 1));
	}

	.hib-fab:hover {
		transform: translateY(-2px);
		box-shadow:
			0 16px 40px rgba(28, 28, 26, 0.28),
			0 0 0 1px rgba(255, 255, 255, 0.08) inset;
	}

	.hib-fab:focus-visible {
		outline: 2px solid var(--wf-accent, #3d5a80);
		outline-offset: 3px;
	}

	.hib-fab-mark {
		display: grid;
		place-items: center;
		width: 2.1rem;
		height: 2.1rem;
		border-radius: 50%;
		background: rgba(244, 242, 236, 0.1);
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.72rem;
		font-weight: 500;
		letter-spacing: -0.04em;
		color: #c5d8ec;
	}

	.hib-fab-text {
		display: flex;
		flex-direction: column;
		gap: 0.05rem;
		min-width: 0;
	}

	.hib-fab-kicker {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.62rem;
		font-weight: 500;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: rgba(244, 242, 236, 0.55);
	}

	.hib-fab-title {
		font-size: 0.92rem;
		font-weight: 600;
		letter-spacing: -0.01em;
		white-space: nowrap;
	}

	.hib-scrim {
		position: fixed;
		inset: 0;
		z-index: 50;
		border: 0;
		padding: 0;
		margin: 0;
		background: rgba(28, 28, 26, 0.42);
		backdrop-filter: blur(2px);
		cursor: pointer;
		animation: hib-fade 0.2s ease;
	}

	.hib-drawer {
		position: fixed;
		z-index: 60;
		top: 0;
		right: 0;
		bottom: 0;
		/* Wide teaching surface — room for code without feeling cramped */
		width: min(46rem, 100vw);
		max-width: 100vw;
		display: flex;
		flex-direction: column;
		background: #161615;
		color: #f0eee8;
		/* No shadow while closed — off-screen panels still paint a haze otherwise */
		box-shadow: none;
		transform: translateX(100%);
		transition:
			transform 0.35s var(--ease, cubic-bezier(0.22, 1, 0.36, 1)),
			box-shadow 0.35s var(--ease, cubic-bezier(0.22, 1, 0.36, 1));
		/* Subtle blueprint grid */
		background-image:
			linear-gradient(rgba(255, 255, 255, 0.03) 1px, transparent 1px),
			linear-gradient(90deg, rgba(255, 255, 255, 0.03) 1px, transparent 1px);
		background-size: 24px 24px;
		background-position: 0 0;
	}

	.hib-drawer.open {
		transform: translateX(0);
		pointer-events: auto;
		box-shadow: -24px 0 64px rgba(0, 0, 0, 0.4);
	}

	.hib-drawer:not(.open) {
		pointer-events: none;
		box-shadow: none;
	}

	.hib-head {
		display: flex;
		align-items: flex-start;
		gap: 0.75rem;
		padding: 1.35rem 1.25rem 1rem;
		border-bottom: 1px solid rgba(244, 242, 236, 0.1);
		background: rgba(0, 0, 0, 0.25);
	}

	.hib-head-text {
		flex: 1;
		min-width: 0;
	}

	.hib-kicker {
		display: inline-block;
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.65rem;
		font-weight: 500;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: #8eb4d4;
		margin-bottom: 0.4rem;
	}

	.hib-title {
		margin: 0 0 0.55rem;
		font-family: var(--wf-serif, Georgia, serif);
		font-size: 1.45rem;
		font-weight: 500;
		letter-spacing: -0.02em;
		line-height: 1.15;
		color: #f6f5f2;
	}

	.hib-summary {
		margin: 0;
		font-size: 0.88rem;
		line-height: 1.5;
		color: rgba(244, 242, 236, 0.72);
	}

	.hib-path {
		margin: 0.65rem 0 0;
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.65rem;
		font-weight: 500;
		letter-spacing: 0.04em;
		color: rgba(142, 180, 212, 0.85);
	}

	.hib-close {
		flex-shrink: 0;
		width: 2.25rem;
		height: 2.25rem;
		border: 1px solid rgba(244, 242, 236, 0.14);
		border-radius: 8px;
		background: rgba(255, 255, 255, 0.04);
		color: rgba(244, 242, 236, 0.85);
		font-size: 1.35rem;
		line-height: 1;
		cursor: pointer;
		transition: background 0.15s ease;
	}

	.hib-close:hover {
		background: rgba(255, 255, 255, 0.1);
	}

	.hib-tabs {
		display: flex;
		flex-wrap: nowrap;
		gap: 0.35rem;
		padding: 0.75rem 1rem;
		overflow-x: auto;
		border-bottom: 1px solid rgba(244, 242, 236, 0.08);
		scrollbar-width: thin;
		background: rgba(0, 0, 0, 0.15);
	}

	.hib-tab {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		flex-shrink: 0;
		padding: 0.45rem 0.75rem;
		border: 1px solid transparent;
		border-radius: 999px;
		background: transparent;
		color: rgba(244, 242, 236, 0.55);
		font-family: var(--wf-sans, system-ui, sans-serif);
		font-size: 0.78rem;
		font-weight: 600;
		cursor: pointer;
		transition:
			background 0.15s ease,
			color 0.15s ease,
			border-color 0.15s ease;
	}

	.hib-tab:hover {
		color: rgba(244, 242, 236, 0.9);
		background: rgba(255, 255, 255, 0.05);
	}

	.hib-tab.active {
		color: #161615;
		background: #e8e4d9;
		border-color: #e8e4d9;
	}

	.hib-tab-n {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.65rem;
		font-weight: 500;
		opacity: 0.65;
	}

	.hib-tab.active .hib-tab-n {
		opacity: 0.85;
		color: #3d5a80;
	}

	.hib-body {
		flex: 1;
		overflow: auto;
		padding: 1.15rem 1.25rem 1.5rem;
	}

	.hib-lede {
		margin: 0 0 1rem;
		font-size: 0.95rem;
		line-height: 1.5;
		color: rgba(244, 242, 236, 0.88);
	}

	.hib-principle {
		margin: 0 0 1.35rem;
		padding: 0.85rem 1rem;
		border-left: 3px solid #3d5a80;
		background: rgba(61, 90, 128, 0.12);
		border-radius: 0 8px 8px 0;
		font-family: var(--wf-serif, Georgia, serif);
		font-size: 0.95rem;
		font-style: italic;
		line-height: 1.4;
		color: #dce8f4;
	}

	.hib-principle-label {
		display: block;
		margin-bottom: 0.25rem;
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.62rem;
		font-style: normal;
		font-weight: 500;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: #8eb4d4;
	}

	.hib-sample {
		margin: 0 0 1.15rem;
		border-radius: 10px;
		overflow: hidden;
		border: 1px solid rgba(244, 242, 236, 0.1);
		background: #0e0e0d;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25);
	}

	.hib-sample-bar {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		padding: 0.55rem 0.85rem;
		background: rgba(255, 255, 255, 0.04);
		border-bottom: 1px solid rgba(244, 242, 236, 0.06);
	}

	.hib-file {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.7rem;
		font-weight: 500;
		color: #9ec5e8;
		word-break: break-all;
	}

	.hib-caption {
		font-size: 0.72rem;
		color: rgba(244, 242, 236, 0.45);
	}

	.hib-code {
		margin: 0;
		padding: 0.95rem 1.05rem 1.15rem;
		overflow-x: auto;
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.78rem;
		line-height: 1.58;
		color: #e8e6e0;
		tab-size: 2;
	}

	.hib-code code {
		font-family: inherit;
		white-space: pre;
	}

	/* Syntax tokens (from highlight.ts) */
	.hib-code :global(.tok-comment) {
		color: #6b7368;
		font-style: italic;
	}
	.hib-code :global(.tok-string) {
		color: #c4a882;
	}
	.hib-code :global(.tok-number) {
		color: #d4a574;
	}
	.hib-code :global(.tok-keyword) {
		color: #7eb0d6;
		font-weight: 500;
	}
	.hib-code :global(.tok-type) {
		color: #9ec5a8;
	}
	.hib-code :global(.tok-fn) {
		color: #d4c07a;
	}
	.hib-code :global(.tok-attr) {
		color: #b89fd4;
	}
	.hib-code :global(.tok-punct) {
		color: #8a8a82;
	}

	.hib-foot {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.75rem 1.25rem;
		border-top: 1px solid rgba(244, 242, 236, 0.1);
		background: rgba(0, 0, 0, 0.3);
		font-size: 0.72rem;
	}

	.hib-foot-label {
		font-family: var(--wf-mono, ui-monospace, monospace);
		letter-spacing: 0.04em;
		color: rgba(244, 242, 236, 0.4);
	}

	.hib-foot-link {
		color: #9ec5e8;
		text-decoration: none;
		font-weight: 600;
	}

	.hib-foot-link:hover {
		text-decoration: underline;
	}

	@keyframes hib-fade {
		from {
			opacity: 0;
		}
		to {
			opacity: 1;
		}
	}

	@media (max-width: 480px) {
		.hib-fab-title {
			max-width: 7rem;
			overflow: hidden;
			text-overflow: ellipsis;
		}
	}
</style>
