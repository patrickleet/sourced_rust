<script lang="ts">
  let { data } = $props();
</script>

<section class="card" style="max-width: 28rem; margin: 2rem auto">
  <h1 style="margin-top: 0">Sign in</h1>
  {#if data.error}
    <p class="error">Auth error: {data.error}</p>
  {/if}
  {#if data.oidcConfigured}
    <p class="muted">
      Continues to your OIDC provider (Zitadel in the Docker stack). Demo humans:
      <code>alice</code> / <code>bob</code> / <code>admin</code> — password
      <code>Password1!</code>
    </p>
    <form method="POST" style="margin-top: 1.25rem">
      <input type="hidden" name="callbackUrl" value={data.callbackUrl} />
      <button class="btn btn-primary" type="submit" style="width: 100%">Continue with OIDC</button>
    </form>
  {:else}
    <p class="muted">
      OIDC is not configured (<code>OIDC_ISSUER</code> / <code>OIDC_CLIENT_ID</code>). Run
      <code>make up</code> and source <code>e2e-ui.env</code>, or use DevHeaders against a local
      runner for offline API tests.
    </p>
    <p class="muted" style="margin-top: 1rem">
      Protected routes require a session. Offline: set
      <code>AUTH_SECRET</code> and complete OIDC bootstrap.
    </p>
  {/if}
</section>
