<script lang="ts">
  let { data } = $props();
  const s = $derived(data.session);
</script>

<h1>Session</h1>
<p class="muted">Auth.js session + tokens used for GraphQL HTTP and WebSocket connection_init.</p>

{#if !s?.user}
  <div class="card">
    <p>Not signed in. <a href="/signin">Sign in</a></p>
  </div>
{:else}
  <div class="card" style="margin-top: 1rem">
    <table class="table">
      <tbody>
        <tr>
          <th>User id (sub)</th>
          <td class="mono">{s.user.id ?? '—'}</td>
        </tr>
        <tr>
          <th>Name</th>
          <td>{s.user.name ?? '—'}</td>
        </tr>
        <tr>
          <th>Email</th>
          <td>{s.user.email ?? '—'}</td>
        </tr>
        <tr>
          <th>Username</th>
          <td class="mono">{s.user.username ?? '—'}</td>
        </tr>
        <tr>
          <th>Groups / roles</th>
          <td class="mono">{(s.user.groups ?? []).join(', ') || '—'}</td>
        </tr>
        <tr>
          <th>GraphQL engine role</th>
          <td><span class="pill">{data.engineRole}</span></td>
        </tr>
        <tr>
          <th>Access token</th>
          <td>
            {#if data.hasAccessToken}
              <span class="pill live">present</span>
              <span class="muted mono">…{(s.accessToken ?? '').slice(-12)}</span>
            {:else}
              <span class="pill warn">missing</span>
            {/if}
          </td>
        </tr>
        <tr>
          <th>Expires at</th>
          <td class="mono">{s.expiresAt ?? '—'}</td>
        </tr>
        {#if s.error}
          <tr>
            <th>Error</th>
            <td class="error">{s.error}</td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
{/if}
