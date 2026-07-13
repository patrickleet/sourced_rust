<script lang="ts">
  let { data } = $props();
</script>

<h1>Catalog</h1>
<p>Session: <code>{data.userId}</code> / <code>{data.role}</code></p>
{#if data.error}
  <p style="color: #a00">API: {data.error}</p>
  <p>Start the fixture: <code>cargo run -p workshop-runner-monolith</code></p>
{:else if data.products.length === 0}
  <p>No products yet. List one via <code>POST /product.list</code>.</p>
{:else}
  <ul>
    {#each data.products as p}
      <li>
        <strong>{p.name}</strong> — {(p.price_cents / 100).toFixed(2)}
        <small>({p.product_id})</small>
      </li>
    {/each}
  </ul>
{/if}
