<script lang="ts">
	/**
	 * Lobby chat — one generated `@load @live` operation.
	 *
	 * SSR, normalized reads, reconnect, and command facts converge through the
	 * package replica. The page declares no cache keys or subscription document.
	 */
	import { tick } from 'svelte';

	import {
		ChatMessages,
		useCommands,
		type Operation_ChatMessages_Data
	} from '$distributed';
	import { Button } from '$lib/components/shared/ui';
	import { AppPage, InlineAlert, PageHeader } from '$lib/components/product';
	import { isOwnAuthor, sessionDisplayName, sessionPrincipalId } from '$lib/session';

	type ChatMsg = Operation_ChatMessages_Data['chat_messages'][number];

	const LOBBY_ROOM = 'lobby';

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

	/** `use()` defaults live because the artifact has a generated companion. */
	const lobby = ChatMessages.use();
	const commands = useCommands();
	const messages = $derived($lobby.complete ? $lobby.data.chat_messages : []);

	async function scrollBottom() {
		await tick();
		if (logEl) logEl.scrollTop = logEl.scrollHeight;
	}

	$effect(() => {
		messages;
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
		return isOwnAuthor(m.author_id, me);
	}

	async function onSend(e: Event) {
		e.preventDefault();
		const body = draft.trim();
		if (!body || busy) return;
		const now = Date.now();
		const message_id = `m-${now.toString(16)}`;
		sendError = null;
		busy = true;
		// Clear only the submitted draft. Anything typed while this command is
		// pending belongs to the next message and must survive its completion.
		draft = '';
		try {
			await commands.chat.post({
				message_id,
				body,
				room_id: LOBBY_ROOM,
				created_at: String(now)
			});
		} catch (error) {
			if (!draft.trim()) draft = body;
			sendError = error instanceof Error ? error.message : 'send failed';
		} finally {
			busy = false;
		}
	}
</script>

<AppPage>
	<PageHeader kicker="Room · {LOBBY_ROOM}" title="Lobby">
		{#snippet meta()}
			<div class="ch-status" data-state={$lobby.live}>
				<span class="ch-pulse" aria-hidden="true"></span>
				{#if $lobby.live === 'active'}
					Live
				{:else if $lobby.live === 'connecting'}
					Connecting…
				{:else if $lobby.live === 'error'}
					Offline
				{:else}
					Idle
				{/if}
			</div>
		{/snippet}
		The generated <code>@load @live</code> artifact owns SSR and reconnect. A typed
		<code>chat.post</code> command updates the same normalized state. Signed in as
		<strong>{displayName}</strong>.
	</PageHeader>

	{#if data.gqlError}
		<InlineAlert label="SSR GraphQL">{data.gqlError}</InlineAlert>
	{/if}
	{#if $lobby.error}
		<InlineAlert label="Subscription">
			{$lobby.error.message}
			<button type="button" class="ch-link-btn" onclick={() => void lobby.refetch()}>Retry</button>
		</InlineAlert>
	{/if}
	{#if sendError}
		<InlineAlert label="Mutation">{sendError}</InlineAlert>
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
					{@const mine = messageIsMine(m)}
					{@const authorLabel = mine ? 'You' : shortId(m.author_id)}
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
			<Button type="submit" variant="ink" disabled={!draft.trim() || busy}>
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
			</Button>
		</form>
	</div>
</AppPage>

<style>
	/* Lobby shell + messages — page-local only */
	.ch-shell {
		--edge: var(--wf-line, #e2e0d9);
		--ink: var(--wf-ink, #1c1c1a);
		--ink-soft: var(--wf-ink-soft, #5c5c56);
		--accent: var(--wf-accent, #3d5a80);
		--surface: var(--wf-bg-elevated, #fff);
		--bubble: #f0efe9;
		--bubble-mine: var(--wf-ink, #1c1c1a);

		display: flex;
		flex-direction: column;
		min-height: min(62vh, 34rem);
		border-radius: var(--df-radius-lg, 10px);
		border: 1px solid var(--edge);
		background: var(--surface);
		overflow: hidden;
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
		border: 1px solid var(--wf-line, #e2e0d9);
		color: var(--wf-ink-soft, #5c5c56);
	}

	.ch-status[data-state='active'] {
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
		color: var(--wf-accent, #3d5a80);
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		border-color: var(--wf-line, #e2e0d9);
	}

	.ch-pulse {
		width: 0.45rem;
		height: 0.45rem;
		border-radius: 50%;
		background: currentColor;
	}

	.ch-status[data-state='active'] .ch-pulse {
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

	.ch-link-btn {
		margin-left: 0.75rem;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		font-weight: 600;
		text-decoration: underline;
		cursor: pointer;
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
