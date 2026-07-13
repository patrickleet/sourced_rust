<script lang="ts">
  let { children } = $props();
</script>

<header
  style="padding: 1rem 1.5rem; border-bottom: 1px solid #e5e5e5; font-family: system-ui, sans-serif; display: flex; gap: 1rem; align-items: center"
>
  <strong>e2e-ui</strong>
  <nav style="display: flex; gap: 0.75rem">
    <a href="/">Todos</a>
    <a href="/chat">Chat</a>
    <a href="/signin">Switch user</a>
  </nav>
</header>
<main style="padding: 1.5rem; font-family: system-ui, sans-serif; max-width: 36rem">
  {@render children()}
</main>
