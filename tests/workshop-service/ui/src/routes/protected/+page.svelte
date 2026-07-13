<script lang="ts">
  let { data } = $props();
</script>

<h1>My orders</h1>
<p>Signed in as <code>{data.userId}</code> ({data.role})</p>
{#if data.error}
  <p style="color: #a00">{data.error}</p>
{:else if data.orders.length === 0}
  <p>No orders for this customer.</p>
{:else}
  <ul>
    {#each data.orders as o}
      <li>{o.order_id}: product {o.product_id} × {o.quantity} — {o.status}</li>
    {/each}
  </ul>
{/if}
