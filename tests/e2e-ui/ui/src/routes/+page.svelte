<script lang="ts">
  import { onMount } from 'svelte';
  import { archiveTodo, completeTodo, createTodo, listTodos, type Todo } from '$lib/api';
  import { readSession } from '$lib/session';

  let session = $state(readSession());
  let todos = $state<Todo[]>([]);
  let title = $state('');
  let error = $state<string | null>(null);
  let loading = $state(true);
  let busyId = $state<string | null>(null);

  /**
   * No GraphQL subscriptions — list is query + poll after commands.
   * Read models project eventually (outbox → bus → projector), so one
   * refetch right after a command often still sees the old row.
   */
  async function refresh(opts?: { quiet?: boolean }) {
    if (!opts?.quiet) loading = true;
    error = null;
    try {
      todos = await listTodos(session);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      if (!opts?.quiet) todos = [];
    } finally {
      if (!opts?.quiet) loading = false;
    }
  }

  async function waitUntil(pred: (list: Todo[]) => boolean, attempts = 40) {
    for (let i = 0; i < attempts; i++) {
      await refresh({ quiet: true });
      if (pred(todos)) return true;
      await new Promise((r) => setTimeout(r, 50));
    }
    return false;
  }

  onMount(() => {
    session = readSession();
    refresh();
  });

  async function onCreate(e: Event) {
    e.preventDefault();
    const t = title.trim();
    if (!t) return;
    try {
      error = null;
      const id = `t-${Date.now().toString(16)}`;
      const created = await createTodo(t, id, session);
      title = '';
      // Optimistic row from command response; then wait for projection.
      todos = [
        {
          todo_id: created.todo_id ?? id,
          owner_id: created.owner_id ?? session.userId,
          title: created.title ?? t,
          status: created.status ?? 'open',
        },
        ...todos,
      ];
      await waitUntil((list) => list.some((x) => x.todo_id === id));
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }

  async function onComplete(id: string) {
    try {
      error = null;
      busyId = id;
      const res = await completeTodo(id, session);
      // Optimistic update from command (aggregate truth); GraphQL catches up via poll.
      todos = todos.map((t) =>
        t.todo_id === id ? { ...t, status: res.status ?? 'completed' } : t
      );
      await waitUntil(
        (list) => list.find((x) => x.todo_id === id)?.status === 'completed'
      );
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      await refresh({ quiet: true });
    } finally {
      busyId = null;
    }
  }

  async function onArchive(id: string) {
    try {
      error = null;
      busyId = id;
      const res = await archiveTodo(id, session);
      todos = todos.map((t) =>
        t.todo_id === id ? { ...t, status: res.status ?? 'archived' } : t
      );
      await waitUntil(
        (list) => list.find((x) => x.todo_id === id)?.status === 'archived'
      );
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      await refresh({ quiet: true });
    } finally {
      busyId = null;
    }
  }
</script>

<h1>My todos</h1>
<p>
  Signed in as <code>{session.userId}</code> ({session.role}). Each user only sees their own
  rows.
</p>
<p style="color: #666; font-size: 0.9rem">
  No subscriptions — after a command the UI updates optimistically, then polls GraphQL until
  the projector has written the read model.
</p>

<form onsubmit={onCreate} style="display: flex; gap: 0.5rem; margin: 1rem 0">
  <input
    bind:value={title}
    placeholder="What needs doing?"
    style="flex: 1; padding: 0.5rem"
  />
  <button type="submit">Add</button>
</form>

{#if error}
  <p style="color: #a00">{error}</p>
  <p><small>API must be up (e.g. <code>make run</code>).</small></p>
{/if}

{#if loading}
  <p>Loading…</p>
{:else if todos.length === 0}
  <p>No todos yet.</p>
{:else}
  <ul style="list-style: none; padding: 0">
    {#each todos as t (t.todo_id)}
      <li
        style="display: flex; gap: 0.5rem; align-items: center; padding: 0.5rem 0; border-bottom: 1px solid #eee"
      >
        <span style:text-decoration={t.status === 'completed' || t.status === 'archived' ? 'line-through' : 'none'}>
          {t.title}
        </span>
        <small style="color: #666">({t.status})</small>
        {#if t.status === 'open'}
          <button
            type="button"
            disabled={busyId === t.todo_id}
            onclick={() => onComplete(t.todo_id)}
          >
            Done
          </button>
        {/if}
        {#if t.status !== 'archived'}
          <button
            type="button"
            disabled={busyId === t.todo_id}
            onclick={() => onArchive(t.todo_id)}
          >
            Archive
          </button>
        {/if}
      </li>
    {/each}
  </ul>
{/if}
