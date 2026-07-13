<script lang="ts">
  let { children } = $props();
</script>

<header style="padding: 1rem; border-bottom: 1px solid #ddd; font-family: system-ui">
  <strong>Workshop UI</strong>
  <nav style="display: inline; margin-left: 1rem">
    <a href="/">Catalog</a>
    |
    <a href="/protected">My orders</a>
    |
    <a href="/signin">Sign in</a>
  </nav>
</header>
<main style="padding: 1.5rem; font-family: system-ui; max-width: 48rem">
  {@render children()}
</main>
