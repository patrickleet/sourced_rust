<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { invalidateAll } from '$app/navigation';
  import { subscribe } from '$lib/graphql-ws';
  import { roleFromGroups } from '$lib/session';

  type ChatMsg = {
    message_id: string;
    room_id: string;
    author_id: string;
    body: string;
    created_at: string;
  };

  let { data, form } = $props();
  let messages = $state<ChatMsg[]>([]);
  let status = $state<'connecting' | 'live' | 'error' | 'idle'>('idle');
  let subError = $state<string | null>(null);
  let unsub: (() => void) | null = null;

  $effect(() => {
    // Seed from SSR data; subscription keeps it live.
    messages = data.messages;
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
    const list = p?.data?.chat_messages;
    if (Array.isArray(list)) {
      messages = [...list].sort((a, b) =>
        a.created_at === b.created_at
          ? a.message_id.localeCompare(b.message_id)
          : a.created_at.localeCompare(b.created_at)
      );
      status = 'live';
      subError = null;
    }
  }

  onMount(() => {
    status = 'connecting';
    const groups = data.session?.user?.groups;
    unsub = subscribe(
      `subscription {
        chat_messages(where: { room_id: { _eq: "${data.room}" } }) {
          message_id room_id author_id body created_at
        }
      }`,
      {
        accessToken: data.accessToken ?? undefined,
        userId: data.userId ?? undefined,
        role: data.engineRole ?? roleFromGroups(groups)
      },
      {
        onNext: applyPayload,
        onError: (e) => {
          status = 'error';
          subError = e instanceof Event ? 'WebSocket error' : String(e);
        },
        onComplete: () => {
          if (status === 'live') status = 'connecting';
        }
      }
    );
  });

  onDestroy(() => unsub?.());
</script>

<h1>Lobby chat</h1>
<p class="muted">
  Initial messages from SSR GraphQL. Live updates via WebSocket subscription with Bearer in
  <code>connection_init</code>.
</p>
<p>
  Status:
  {#if status === 'live'}
    <span class="pill live">live</span>
  {:else if status === 'connecting'}
    <span class="pill warn">connecting</span>
  {:else if status === 'error'}
    <span class="pill" style="background: rgba(240,113,120,0.15); color: var(--danger)">error</span>
  {:else}
    <span class="pill">idle</span>
  {/if}
</p>

{#if data.gqlError}
  <p class="error">SSR GraphQL: {data.gqlError}</p>
{/if}
{#if subError}
  <p class="error">Subscription: {subError}</p>
{/if}
{#if form?.message}
  <p class="error">{form.message}</p>
{/if}

<div class="chat-log">
  {#if messages.length === 0}
    <p class="muted">No messages yet — say hello.</p>
  {:else}
    {#each messages as m (m.message_id)}
      <div class="chat-msg">
        <strong>{m.author_id}</strong>
        <span class="when">{m.created_at}</span>
        <div>{m.body}</div>
      </div>
    {/each}
  {/if}
</div>

<form
  method="POST"
  action="?/post"
  class="field"
  onsubmit={() => {
    // After progressive enhancement, invalidate to re-SSR if sub lags.
    setTimeout(() => invalidateAll(), 200);
  }}
>
  <input name="body" placeholder="Message the lobby…" required autocomplete="off" />
  <button class="btn btn-primary" type="submit">Send</button>
</form>
