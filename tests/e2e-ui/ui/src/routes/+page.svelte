<script lang="ts">
  let { data } = $props();
</script>

<section class="hero">
  <span class="pill">Distributed template</span>
  <h1>Personal todos & live chat on real OIDC</h1>
  <p>
    Fieldnote is the e2e-ui fixture: multi-crate CQRS domain, GraphQL with row-level filters,
    WebSocket subscriptions, and Auth.js → Zitadel (or any OIDC) against a Postgres-backed
    Distributed service.
  </p>
  <div style="display: flex; gap: 0.65rem; flex-wrap: wrap">
    {#if data.session?.user}
      <a class="btn btn-primary" href="/todos">Open todos</a>
      <a class="btn btn-ghost" href="/chat">Lobby chat</a>
    {:else}
      <a class="btn btn-primary" href="/signin">Sign in with OIDC</a>
      <a class="btn btn-ghost" href="/session">Session</a>
    {/if}
  </div>
</section>

<div class="grid-2" style="margin-top: 1.5rem">
  <div class="feature">
    <h3>Owner-scoped todos</h3>
    <p>
      Commands take identity from the access token; GraphQL filters
      <code>owner_id = claim(x-user-id)</code>. Projectors only write read models.
    </p>
  </div>
  <div class="feature">
    <h3>Live chat subscriptions</h3>
    <p>
      <code>subscription {'{'} chat_messages {'}'}</code> over
      <code>/graphql/ws</code> with Bearer in <code>connection_init</code> — the browser-safe
      pattern.
    </p>
  </div>
  <div class="feature">
    <h3>SSR GraphQL</h3>
    <p>
      Protected pages load on the server with the session access token so a hard refresh paints
      data, not a client-only Loading spinner.
    </p>
  </div>
  <div class="feature">
    <h3>Docker template</h3>
    <p>
      <code>make up</code> starts Postgres + Zitadel, bootstraps users/roles, and writes
      <code>e2e-ui.env</code> for the runner and UI.
    </p>
  </div>
</div>
