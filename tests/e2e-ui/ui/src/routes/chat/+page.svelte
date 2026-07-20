<script lang="ts">
	/**
	 * Lobby chat — Houdini-style document store (cache is transparent).
	 * Live list: `gql.live(...)` → `$lobby.data` / `$lobby.status`
	 * Posts: command pipeline with optimistic row against the same store document.
	 */
	import { onDestroy, tick } from 'svelte';
	import { useGraphql, fx } from '$lib/gql';
	import { isOwnAuthor, sessionDisplayName, sessionPrincipalId } from '$lib/session';
	import { chat, sortChatMessages } from './chat.resource';
	import type { ChatMsg } from './chat.resource';

	let { data } = $props();
	let sendError = $state<string | null>(null);
	let logEl: HTMLDivElement | undefined = $state();
	let draft = $state('');
	let busy = $state(false);

	/** Same principal the API stamps as author_id (access-token sub). */
	const me = $derived(
		sessionPrincipalId(data.session, data.accessToken ?? data.session?.accessToken)
	);
	const displayName = $derived(sessionDisplayName(data.session));

	const gql = useGraphql(() => data, {
		runEffects: (effects) => {
			for (const e of effects) {
				if (e.kind === 'alert') sendError = e.message;
			}
		}
	});

	const subDoc = chat.subscription ?? chat.query;

	/** Cache + live subscription — no manual seed/subscribe/sync. */
	const lobby = gql.live({
		document: subDoc,
		list: { at: 'chat_messages', by: 'message_id' },
		initialData: { chat_messages: data.messages ?? [] },
		select: (d: { chat_messages?: ChatMsg[] }) =>
			sortChatMessages(d?.chat_messages ?? [])
	});

	// Keep seed in sync if load re-runs (navigation / invalidate).
	$effect(() => {
		lobby.seed({ chat_messages: data.messages ?? [] });
	});

	onDestroy(() => lobby.destroy());

	async function scrollBottom() {
		await tick();
		if (logEl) logEl.scrollTop = logEl.scrollHeight;
	}

	$effect(() => {
		$lobby.data;
		void scrollBottom();
	});

	function shortId(id: string) {
		if (!id) return '?';
		if (id.length <= 10) return id;
		return id.slice(0, 6) + '…';
	}

	function formatWhen(raw: string) {
		if (!raw) return '';
		// Server may emit unix millis as a decimal string (no chrono in fixture).
		const asNum = /^\d{11,16}$/.test(raw.trim()) ? Number(raw) : NaN;
		const d = Number.isFinite(asNum) ? new Date(asNum) : new Date(raw);
		if (Number.isNaN(d.getTime())) return raw;
		return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	}

	function messageIsMine(m: ChatMsg): boolean {
		return isOwnAuthor(m.author_id, me, {
			authorUserId: m.author?.user_id,
			username: data.session?.user?.username,
			displayName: m.author?.display_name
		});
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
			created_at: new Date().toISOString(),
			// Join target filled by server when auth_users has this user_id.
			author: {
				user_id: me || 'me',
				display_name: displayName || 'You',
				email: '',
				status: 'active'
			}
		};
		const result = await gql.commands.chatMessagesPost(
			{ message_id, body, room_id: data.room },
			{
				// Policies default fact + subscription; sub is already live.
				optimistic: {
					targets: [lobby.target('chat_messages', 'message_id')],
					row: optimisticRow
				},
				onError: ({ errors }) => [fx.alert(errors[0]?.message ?? 'send failed')]
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
			<div class="ch-status" data-state={$lobby.status}>
				<span class="ch-pulse" aria-hidden="true"></span>
				{#if $lobby.status === 'live'}
					Live
				{:else if $lobby.status === 'connecting'}
					Connecting…
				{:else if $lobby.status === 'error'}
					Offline
				{:else}
					Idle
				{/if}
			</div>
		</div>
		<p class="ch-lede">
			<code>gql.live</code> owns the list (cache + subscription write-through). Posts use
			<code>gql.commands.*</code> with optimistic rows against the same document. Signed in as
			<strong>{displayName}</strong>.
		</p>
	</header>

	{#if data.gqlError}
		<div class="ch-alert" role="alert">
			<strong>SSR GraphQL</strong>
			<span>{data.gqlError}</span>
		</div>
	{/if}
	{#if $lobby.error}
		<div class="ch-alert" role="alert">
			<strong>Subscription</strong>
			<span>{$lobby.error}</span>
			<button type="button" class="ch-link-btn" onclick={() => lobby.connect()}>Retry</button>
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
			{#if $lobby.data.length === 0}
				<div class="ch-empty">
					<div class="ch-empty-icon" aria-hidden="true">◇</div>
					<p>No messages yet. Say hello to the lobby.</p>
				</div>
			{:else}
				{#each $lobby.data as m, i (m.message_id)}
					{@const mine = messageIsMine(m)}
					{@const authorLabel = mine
						? 'You'
						: m.author?.display_name || m.author?.email || shortId(m.author_id)}
					<article class="ch-msg" class:mine style="--i: {i}">
						<header class="ch-msg-meta">
							<span class="ch-author" title={m.author_id}>{authorLabel}</span>
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
		--ink: var(--wf-ink, #1c1c1a);
		--ink-soft: var(--wf-ink-soft, #5c5c56);
		--surface: var(--wf-bg-elevated, #fff);
		--bubble: #fff;
		--bubble-mine: var(--wf-ink, #1c1c1a);
		--edge: var(--wf-line, #e2e0d9);
		--accent: var(--wf-accent, #3d5a80);

		max-width: 42rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 3.5rem;
		font-family: var(--wf-sans, system-ui, sans-serif);
		color: var(--ink);
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
		font-weight: 600;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--ink-soft);
		margin-bottom: 0.4rem;
	}

	.ch-title {
		margin: 0;
		font-family: var(--wf-serif, Georgia, serif);
		font-size: clamp(1.65rem, 4vw, 2.15rem);
		font-weight: 500;
		letter-spacing: -0.02em;
		line-height: 1.1;
	}

	.ch-status {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.35rem 0.7rem;
		border-radius: 999px;
		font-size: 0.72rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		background: transparent;
		border: 1px solid var(--edge);
		color: var(--ink-soft);
	}

	.ch-status[data-state='live'] {
		color: var(--wf-success, #2f6f4e);
		background: rgba(47, 111, 78, 0.1);
		border-color: rgba(47, 111, 78, 0.22);
	}

	.ch-status[data-state='error'] {
		color: var(--wf-danger, #b33a3a);
		background: rgba(179, 58, 58, 0.08);
		border-color: rgba(179, 58, 58, 0.22);
	}

	.ch-status[data-state='connecting'] {
		color: var(--accent);
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		border-color: var(--edge);
	}

	.ch-pulse {
		width: 0.45rem;
		height: 0.45rem;
		border-radius: 50%;
		background: currentColor;
	}

	.ch-status[data-state='live'] .ch-pulse {
		box-shadow: 0 0 0 0 rgba(47, 111, 78, 0.45);
		animation: ch-pulse 1.6s ease infinite;
	}

	@keyframes ch-pulse {
		70% {
			box-shadow: 0 0 0 7px transparent;
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
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.85em;
		padding: 0.08em 0.3em;
		border-radius: 4px;
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
	}

	.ch-alert {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem 0.75rem;
		padding: 0.75rem 0.95rem;
		margin-bottom: 0.85rem;
		border-radius: var(--wf-radius, 6px);
		background: rgba(179, 58, 58, 0.08);
		border: 1px solid rgba(179, 58, 58, 0.22);
		color: var(--wf-danger, #b33a3a);
		font-size: 0.9rem;
	}

	.ch-link-btn {
		margin-left: auto;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		font-weight: 600;
		text-decoration: underline;
		cursor: pointer;
	}

	.ch-shell {
		display: flex;
		flex-direction: column;
		min-height: min(62vh, 34rem);
		border-radius: var(--df-radius-lg, 10px);
		border: 1px solid var(--edge);
		background: var(--surface);
		box-shadow: none;
		overflow: hidden;
	}

	.ch-log {
		flex: 1;
		overflow-y: auto;
		padding: 1.1rem 1rem 0.75rem;
		display: flex;
		flex-direction: column;
		gap: 0.55rem;
		scroll-behavior: smooth;
	}

	.ch-empty {
		margin: auto;
		text-align: center;
		color: var(--ink-soft);
		padding: 2rem 1rem;
	}

	.ch-empty-icon {
		font-size: 1.35rem;
		opacity: 0.35;
		margin-bottom: 0.5rem;
	}

	.ch-msg {
		max-width: min(88%, 28rem);
		align-self: flex-start;
		padding: 0.6rem 0.8rem 0.7rem;
		border-radius: 10px 10px 10px 4px;
		background: var(--bubble);
		border: 1px solid var(--edge);
	}

	.ch-msg.mine {
		align-self: flex-end;
		border-radius: 10px 10px 4px 10px;
		background: var(--bubble-mine);
		border-color: transparent;
		color: #f6f5f2;
	}

	.ch-msg-meta {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.75rem;
		margin-bottom: 0.2rem;
	}

	.ch-author {
		font-size: 0.7rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		opacity: 0.65;
	}

	.ch-when {
		font-size: 0.68rem;
		font-variant-numeric: tabular-nums;
		opacity: 0.5;
	}

	.ch-body {
		margin: 0;
		font-size: 0.95rem;
		line-height: 1.45;
		white-space: pre-wrap;
		word-break: break-word;
	}

	.ch-composer {
		display: flex;
		gap: 0.5rem;
		padding: 0.75rem;
		border-top: 1px solid var(--edge);
		background: rgba(28, 28, 26, 0.02);
	}

	.ch-input {
		flex: 1;
		min-width: 0;
		border: 1px solid var(--edge);
		border-radius: var(--wf-radius, 6px);
		padding: 0.65rem 0.85rem;
		font: inherit;
		font-size: 0.95rem;
		background: #fff;
		color: var(--ink);
		outline: none;
		transition:
			border-color 0.15s ease,
			box-shadow 0.15s ease;
	}

	.ch-input:focus {
		border-color: var(--accent);
		box-shadow: 0 0 0 3px var(--wf-accent-soft, rgba(61, 90, 128, 0.12));
	}

	.ch-send {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		border: none;
		border-radius: var(--wf-radius, 6px);
		padding: 0 1rem;
		font: inherit;
		font-weight: 600;
		font-size: 0.9rem;
		cursor: pointer;
		background: var(--ink);
		color: #fff;
		transition: background 0.15s ease, opacity 0.15s ease;
	}

	.ch-send:hover:not(:disabled) {
		background: #2a2a28;
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
