<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { readSession } from '$lib/session';
  import { identityHeaders } from '$lib/session';
  import { subscribe } from '$lib/graphql-ws';

  type ChatMsg = {
    message_id: string;
    room_id: string;
    author_id: string;
    body: string;
    created_at: string;
  };

  const ROOM = 'lobby';
  let session = $state(readSession());
  let messages = $state<ChatMsg[]>([]);
  let body = $state('');
  let error = $state<string | null>(null);
  let status = $state<'connecting' | 'live' | 'error'>('connecting');
  let unsub: (() => void) | null = null;

  function applyPayload(payload: unknown) {
    const p = payload as {
      data?: { chat_messages?: ChatMsg[] };
      errors?: Array<{ message: string }>;
    };
    if (p?.errors?.length) {
      error = p.errors[0].message;
      return;
    }
    const list = p?.data?.chat_messages;
    if (Array.isArray(list)) {
      // Server may not order; sort by created_at then id.
      messages = [...list].sort((a, b) =>
        a.created_at === b.created_at
          ? a.message_id.localeCompare(b.message_id)
          : a.created_at.localeCompare(b.created_at)
      );
      status = 'live';
      error = null;
    }
  }

  function connect() {
    unsub?.();
    status = 'connecting';
    session = readSession();
    unsub = subscribe(
      `subscription {
        chat_messages(where: { room_id: { _eq: "${ROOM}" } }) {
          message_id room_id author_id body created_at
        }
      }`,
      session,
      {
        onNext: applyPayload,
        onError: (e) => {
          status = 'error';
          error = e instanceof Event ? 'WebSocket error' : String(e);
        },
        onComplete: () => {
          if (status === 'live') status = 'connecting';
        },
      }
    );
  }

  onMount(() => {
    connect();
  });

  onDestroy(() => {
    unsub?.();
  });

  async function onSend(e: Event) {
    e.preventDefault();
    const text = body.trim();
    if (!text) return;
    const message_id = `m-${Date.now().toString(16)}`;
    try {
      error = null;
      const res = await fetch('/chat.post', {
        method: 'POST',
        headers: identityHeaders(session),
        body: JSON.stringify({ message_id, body: text, room_id: ROOM }),
      });
      const json = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(json.error ?? `HTTP ${res.status}`);
      body = '';
      // List updates via subscription push after projector commit — no poll.
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }
</script>

<h1>Lobby chat</h1>
<p>
  Signed in as <code>{session.userId}</code>. Messages are shared in room
  <code>{ROOM}</code>.
</p>
<p style="color: #666; font-size: 0.9rem">
  Live list via GraphQL <strong>subscription</strong> on
  <code>chat_messages</code> (WebSocket <code>/graphql/ws</code>). Open this page
  in two browsers with different users to see pushes.
</p>
<p>
  Status:
  {#if status === 'live'}
    <span style="color: #080">live</span>
  {:else if status === 'connecting'}
    <span style="color: #a60">connecting…</span>
  {:else}
    <span style="color: #a00">error</span>
  {/if}
</p>

{#if error}
  <p style="color: #a00">{error}</p>
{/if}

<div
  style="border: 1px solid #ddd; border-radius: 6px; min-height: 16rem; max-height: 24rem; overflow: auto; padding: 0.75rem; margin: 1rem 0; background: #fafafa"
>
  {#if messages.length === 0}
    <p style="color: #888">No messages yet — say hi.</p>
  {:else}
    {#each messages as m (m.message_id)}
      <div style="margin-bottom: 0.5rem">
        <strong>{m.author_id}</strong>
        <small style="color: #888"> {m.created_at}</small>
        <div>{m.body}</div>
      </div>
    {/each}
  {/if}
</div>

<form onsubmit={onSend} style="display: flex; gap: 0.5rem">
  <input
    bind:value={body}
    placeholder="Message the lobby…"
    style="flex: 1; padding: 0.5rem"
    autocomplete="off"
  />
  <button type="submit">Send</button>
</form>
