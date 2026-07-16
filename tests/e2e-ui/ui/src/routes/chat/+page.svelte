<script lang="ts">
	/**
	 * Lobby chat — co-located `chat` resource + command pipeline.
	 * Posts: `gql.commands.chatMessagesPost` (optimistic + fact; reconcile via subscription).
	 * Live: `gql.subscribe` write-through into QueryCache.
	 */
	import { onDestroy, onMount, tick } from 'svelte';
	import {
		useGraphql,
		effect,
		listTarget,
		seedQueryCache,
		readQueryList,
		queryDocString
	} from '$lib/gql';
	import { sessionDisplayName } from '$lib/session';
	import { chat, sortChatMessages } from './chat.resource';
	import type { ChatMsg } from './chat.resource';

	let { data } = $props();
	let messages = $state<ChatMsg[]>([...(data.messages ?? [])]);
	let status = $state<'connecting' | 'live' | 'error' | 'idle'>('idle');
	let subError = $state<string | null>(null);
	let sendError = $state<string | null>(null);
	let unsub: (() => void) | null = null;
	let unsubCache: (() => void) | null = null;
	let logEl: HTMLDivElement | undefined = $state();
	let draft = $state('');
	let busy = $state(false);

	const me = $derived(data.userId ?? data.session?.user?.id ?? '');
	const displayName = $derived(sessionDisplayName(data.session));

	const gql = useGraphql(() => data, {
		runEffects: (effects) => {
			for (const e of effects) {
				if (e.kind === 'alert') sendError = e.message;
			}
		}
	});

	const subDoc = chat.subscription ?? chat.query;
	const chatTarget = listTarget(subDoc, 'chat_messages', 'message_id');

	function syncFromCache() {
		const list = readQueryList<ChatMsg>(gql.cache, subDoc, 'chat_messages');
		messages = sortChatMessages(list.length ? list : (data.messages ?? []));
	}

	async function scrollBottom() {
		await tick();
		if (logEl) logEl.scrollTop = logEl.scrollHeight;
	}

	$effect(() => {
		messages;
		void scrollBottom();
	});

	function applyPayload(payload: unknown) {
		const p = payload as {
			data?: { chat_messages?: ChatMsg[] };
			errors?: Array<{ message: string }>;
		};
		if (p?.errors?.length) {
			subError = p.errors[0].message;
			status = 'error';
			return;
		}
		// subscribe write-through already updated cache; sync UI.
		if (p?.data?.chat_messages) {
			status = 'live';
			subError = null;
			syncFromCache();
		}
	}

	function connect() {
		unsub?.();
		status = 'connecting';
		subError = null;
		unsub = gql.subscribe(subDoc, {
			onNext: applyPayload,
			onError: (e) => {
				status = 'error';
				if (e instanceof Event) subError = 'WebSocket error — is the API running on :8791?';
				else if (Array.isArray(e)) subError = JSON.stringify(e);
				else subError = String(e);
			},
			onComplete: () => {
				if (status === 'live') status = 'connecting';
			}
		});
	}

	onMount(() => {
		const key = seedQueryCache(gql.cache, subDoc, {
			chat_messages: data.messages ?? []
		});
		syncFromCache();
		unsubCache = gql.cache.subscribe(key, () => syncFromCache());
		connect();
	});

	onDestroy(() => {
		unsub?.();
		unsubCache?.();
	});

	function shortId(id: string) {
		if (!id) return '?';
		if (id.length <= 10) return id;
		return id.slice(0, 6) + '…';
	}

	function formatWhen(iso: string) {
		try {
			const d = new Date(iso);
			if (Number.isNaN(d.getTime())) return iso;
			return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
		} catch {
			return iso;
		}
	}

	async function onSend(e: Event) {
		e.preventDefault();
		const body = draft.trim();
		if (!body || busy) return;
		const message_id = `m-${Date.now().toString(16)}`;
		sendError = null;
		busy = true;
		const optimisticRow = {
			message_id,
			room_id: data.room,
			author_id: me || 'me',
			body,
			created_at: new Date().toISOString()
		};
		const result = await gql.commands.chatMessagesPost(
			{
				message_id,
				body,
				room_id: data.room
			},
			{
				// Fact-shaped payload; live list truth from subscription write-through.
				result: { kind: 'fact' },
				reconcile: { kind: 'subscription', document: queryDocString(subDoc) },
				optimistic: {
					targets: [chatTarget],
					row: optimisticRow
				},
				onError: ({ errors }) => [effect.alert(errors[0]?.message ?? 'send failed')]
			}
		);
		busy = false;
		if (result.errors?.length || !result.data) {
			if (!sendError) sendError = result.errors?.[0]?.message ?? 'send failed';
			return;
		}
		draft = '';
	}
</script>

<section class="ch-page">
	<header class="ch-header">
		<div class="ch-title-row">
			<div>
				<div class="ch-kicker">Room · {data.room}</div>
				<h1 class="ch-title">Lobby</h1>
			</div>
			<div class="ch-status" data-state={status}>
				<span class="ch-pulse" aria-hidden="true"></span>
				{#if status === 'live'}
					Live
				{:else if status === 'connecting'}
					Connecting…
				{:else if status === 'error'}
					Offline
				{:else}
					Idle
				{/if}
			</div>
		</div>
		<p class="ch-lede">
			Co-located <code>chat.resource</code>: SSR seed + live
			<code>gql.subscribe</code> (cache write-through) +
			<code>gql.commands.chatMessagesPost</code> pipeline. Signed in as
			<strong>{displayName}</strong>.
		</p>
	</header>

	{#if data.gqlError}
		<div class="ch-alert" role="alert">
			<strong>SSR GraphQL</strong>
			<span>{data.gqlError}</span>
		</div>
	{/if}
	{#if subError}
		<div class="ch-alert" role="alert">
			<strong>Subscription</strong>
			<span>{subError}</span>
			<button type="button" class="ch-link-btn" onclick={connect}>Retry</button>
		</div>
	{/if}
	{#if sendError}
		<div class="ch-alert" role="alert">
			<strong>Mutation</strong>
			<span>{sendError}</span>
		</div>
	{/if}

	<div class="ch-shell">
		<div class="ch-log" bind:this={logEl} role="log" aria-live="polite" aria-relevant="additions">
			{#if messages.length === 0}
				<div class="ch-empty">
					<div class="ch-empty-icon" aria-hidden="true">◇</div>
					<p>No messages yet. Say hello to the lobby.</p>
				</div>
			{:else}
				{#each messages as m, i (m.message_id)}
					{@const mine = me && m.author_id === me}
					<article class="ch-msg" class:mine style="--i: {i}">
						<header class="ch-msg-meta">
							<span class="ch-author">{mine ? 'You' : shortId(m.author_id)}</span>
							<time class="ch-when" datetime={m.created_at}>{formatWhen(m.created_at)}</time>
						</header>
						<p class="ch-body">{m.body}</p>
					</article>
				{/each}
			{/if}
		</div>

		<form class="ch-composer" onsubmit={onSend}>
			<label class="ch-sr" for="chat-body">Message</label>
			<input
				id="chat-body"
				class="ch-input"
				name="body"
				placeholder="Message the lobby…"
				required
				autocomplete="off"
				bind:value={draft}
			/>
			<button class="ch-send" type="submit" disabled={!draft.trim() || busy}>
				Send
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
					<path
						d="M5 12h14M13 6l6 6-6 6"
						stroke="currentColor"
						stroke-width="2.4"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
				</svg>
			</button>
		</form>
	</div>
</section>

<style>
	.ch-page {
		--ink: var(--hops-navy, #1a2744);
		--ink-soft: rgba(26, 39, 68, 0.62);
		--amber: var(--hops-orange, #e69a2d);
		--surface: rgba(255, 255, 255, 0.78);
		--bubble: #ffffff;
		--bubble-mine: linear-gradient(145deg, #1a2744 0%, #2a3a5c 100%);
		--edge: rgba(26, 39, 68, 0.1);

		max-width: 42rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 3.5rem;
		font-family: var(--font-body, 'Lexend', system-ui, sans-serif);
		color: var(--ink);
		animation: ch-in 0.5s var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1)) both;
	}

	@keyframes ch-in {
		from {
			opacity: 0;
			transform: translateY(10px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	.ch-header {
		margin-bottom: 1.25rem;
	}

	.ch-title-row {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 1rem;
		flex-wrap: wrap;
	}

	.ch-kicker {
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--ink-soft);
		margin-bottom: 0.4rem;
	}

	.ch-title {
		margin: 0;
		font-size: clamp(1.85rem, 4vw, 2.4rem);
		font-weight: 800;
		letter-spacing: -0.03em;
		line-height: 1.1;
	}

	.ch-status {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		padding: 0.4rem 0.75rem;
		border-radius: 999px;
		font-size: 0.75rem;
		font-weight: 700;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		background: rgba(26, 39, 68, 0.06);
		border: 1px solid var(--edge);
		color: var(--ink-soft);
	}

	.ch-status[data-state='live'] {
		color: #276749;
		background: rgba(56, 161, 105, 0.12);
		border-color: rgba(56, 161, 105, 0.28);
	}

	.ch-status[data-state='error'] {
		color: #9b2c2c;
		background: rgba(229, 62, 62, 0.1);
		border-color: rgba(229, 62, 62, 0.25);
	}

	.ch-status[data-state='connecting'] {
		color: #975a16;
		background: rgba(230, 154, 45, 0.12);
		border-color: rgba(230, 154, 45, 0.3);
	}

	.ch-pulse {
		width: 0.5rem;
		height: 0.5rem;
		border-radius: 50%;
		background: currentColor;
	}

	.ch-status[data-state='live'] .ch-pulse {
		box-shadow: 0 0 0 0 rgba(56, 161, 105, 0.5);
		animation: ch-pulse 1.6s ease infinite;
	}

	@keyframes ch-pulse {
		70% {
			box-shadow: 0 0 0 8px transparent;
		}
		100% {
			box-shadow: 0 0 0 0 transparent;
		}
	}

	.ch-lede {
		margin: 0.75rem 0 0;
		font-size: 0.95rem;
		line-height: 1.5;
		color: var(--ink-soft);
	}

	.ch-lede code {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.85em;
		padding: 0.08em 0.3em;
		border-radius: 4px;
		background: rgba(26, 39, 68, 0.06);
	}

	.ch-alert {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem 0.75rem;
		padding: 0.75rem 0.95rem;
		margin-bottom: 0.85rem;
		border-radius: 12px;
		background: rgba(229, 62, 62, 0.08);
		border: 1px solid rgba(229, 62, 62, 0.22);
		color: #9b2c2c;
		font-size: 0.9rem;
	}

	.ch-link-btn {
		margin-left: auto;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		font-weight: 700;
		text-decoration: underline;
		cursor: pointer;
	}

	.ch-shell {
		display: flex;
		flex-direction: column;
		min-height: min(62vh, 34rem);
		border-radius: 20px;
		border: 1px solid var(--edge);
		background: var(--surface);
		backdrop-filter: blur(12px);
		box-shadow:
			0 20px 50px rgba(15, 24, 41, 0.1),
			0 1px 0 rgba(255, 255, 255, 0.7) inset;
		overflow: hidden;
	}

	.ch-log {
		flex: 1;
		overflow-y: auto;
		padding: 1.15rem 1.1rem 0.75rem;
		display: flex;
		flex-direction: column;
		gap: 0.65rem;
		scroll-behavior: smooth;
	}

	.ch-empty {
		margin: auto;
		text-align: center;
		color: var(--ink-soft);
		padding: 2rem 1rem;
	}

	.ch-empty-icon {
		font-size: 1.5rem;
		opacity: 0.4;
		margin-bottom: 0.5rem;
	}

	.ch-msg {
		max-width: min(88%, 28rem);
		align-self: flex-start;
		padding: 0.65rem 0.85rem 0.75rem;
		border-radius: 14px 14px 14px 4px;
		background: var(--bubble);
		border: 1px solid var(--edge);
		box-shadow: 0 4px 14px rgba(15, 24, 41, 0.05);
		animation: ch-msg 0.35s var(--ease-out-expo, ease) both;
		animation-delay: calc(min(var(--i, 0), 12) * 20ms);
	}

	.ch-msg.mine {
		align-self: flex-end;
		border-radius: 14px 14px 4px 14px;
		background: var(--bubble-mine);
		border-color: transparent;
		color: #f8fafc;
		box-shadow: 0 8px 22px rgba(26, 39, 68, 0.22);
	}

	@keyframes ch-msg {
		from {
			opacity: 0;
			transform: translateY(6px) scale(0.98);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	.ch-msg-meta {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.75rem;
		margin-bottom: 0.25rem;
	}

	.ch-author {
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		opacity: 0.7;
	}

	.ch-when {
		font-size: 0.68rem;
		font-variant-numeric: tabular-nums;
		opacity: 0.55;
	}

	.ch-body {
		margin: 0;
		font-size: 0.98rem;
		line-height: 1.45;
		white-space: pre-wrap;
		word-break: break-word;
	}

	.ch-composer {
		display: flex;
		gap: 0.55rem;
		padding: 0.85rem;
		border-top: 1px solid var(--edge);
		background: rgba(248, 249, 252, 0.9);
	}

	.ch-input {
		flex: 1;
		min-width: 0;
		border: 1px solid var(--edge);
		border-radius: 12px;
		padding: 0.75rem 0.95rem;
		font: inherit;
		font-size: 0.98rem;
		background: #fff;
		color: var(--ink);
		outline: none;
		transition: border-color 0.15s ease, box-shadow 0.15s ease;
	}

	.ch-input:focus {
		border-color: rgba(230, 154, 45, 0.55);
		box-shadow: 0 0 0 3px rgba(230, 154, 45, 0.18);
	}

	.ch-send {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		border: none;
		border-radius: 12px;
		padding: 0 1.1rem;
		font: inherit;
		font-weight: 700;
		font-size: 0.9rem;
		cursor: pointer;
		background: var(--ink);
		color: #fff;
		transition:
			opacity 0.15s ease,
			transform 0.15s ease,
			background 0.15s ease;
	}

	.ch-send:hover:not(:disabled) {
		background: var(--hops-navy-light, #2a3a5c);
	}

	.ch-send:active:not(:disabled) {
		transform: scale(0.97);
	}

	.ch-send:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}

	.ch-sr {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		border: 0;
	}
</style>
