<script lang="ts">
  import '../app.css';
  import { page } from '$app/stores';

  let { data, children } = $props();

  function current(path: string) {
    return $page.url.pathname === path || $page.url.pathname.startsWith(path + '/');
  }
</script>

<div class="shell">
  <header class="topbar">
    <a class="brand" href="/">
      <span class="brand-mark" aria-hidden="true"></span>
      Fieldnote
    </a>
    <nav class="nav">
      <a href="/" aria-current={current('/') && $page.url.pathname === '/' ? 'page' : undefined}
        >Home</a
      >
      <a href="/todos" aria-current={current('/todos') ? 'page' : undefined}>Todos</a>
      <a href="/chat" aria-current={current('/chat') ? 'page' : undefined}>Chat</a>
      <a href="/session" aria-current={current('/session') ? 'page' : undefined}>Session</a>
      {#if data.session?.user}
        <span class="muted mono">{data.session.user.name ?? data.session.user.email ?? 'signed in'}</span>
        <a href="/auth/signout">Sign out</a>
      {:else}
        <a href="/signin">Sign in</a>
      {/if}
    </nav>
  </header>
  {@render children()}
</div>
