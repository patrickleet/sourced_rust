<script lang="ts">
  let { data, form } = $props();
</script>

<h1>My todos</h1>
<p class="muted">
  SSR GraphQL with your access token. Rows are filtered to your
  <code>sub</code> as <code>owner_id</code>.
</p>

{#if data.gqlError}
  <p class="error">GraphQL: {data.gqlError}</p>
{/if}
{#if form?.message}
  <p class="error">{form.message}</p>
{/if}

<form method="POST" action="?/create" class="field">
  <input name="title" placeholder="What needs doing?" required autocomplete="off" />
  <button class="btn btn-primary" type="submit">Add</button>
</form>

<div class="card">
  {#if data.todos.length === 0}
    <p class="muted" style="margin: 0">No todos yet — add one above.</p>
  {:else}
    <ul class="list">
      {#each data.todos as t (t.todo_id)}
        <li>
          <span class={t.status !== 'open' ? 'strike' : ''}>{t.title}</span>
          <span class="pill">{t.status}</span>
          {#if t.status === 'open'}
            <form method="POST" action="?/complete" style="margin-left: auto; display: inline">
              <input type="hidden" name="todo_id" value={t.todo_id} />
              <button class="btn btn-ghost" type="submit">Done</button>
            </form>
          {/if}
          {#if t.status !== 'archived'}
            <form method="POST" action="?/archive" style="display: inline">
              <input type="hidden" name="todo_id" value={t.todo_id} />
              <button class="btn btn-ghost" type="submit">Archive</button>
            </form>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
