<script lang="ts">
	/**
	 * Unauthenticated lobby peek path.
	 *
	 * Does not use `$distributed` (that client is the portable user contract).
	 * Call GraphQL with the **e2e-ui-public** application surface and empty
	 * identity when the edge allows unauthenticated access (DevHeaders / OIDC
	 * require_auth=false). See `e2e_service::distributed_public_client_surface`
	 * and the service unit test `public_surface_opens_and_queries_chat_without_identity`.
	 */
	import { AppPage, PageHeader } from '$lib/components/product';

	const SAMPLE = `{
  "query": "{ chat_messages(limit: 10, offset: 0) { message_id body room_id created_at } }",
  "extensions": {
    "distributed": {
      "client": {
        "surface": {
          "kind": "application",
          "name": "e2e-ui-public",
          "roles": ["anonymous"]
        },
        "schemaHash": "<from dctl client-manifest --entrypoint e2e_service::distributed_public_client_surface>"
      }
    }
  }
}`;
</script>

<AppPage>
	<PageHeader title="Public lobby (anonymous)" />
	<p class="lead">
		This route is intentionally unauthenticated. It documents the bare protocol path for the
		<strong>e2e-ui-public</strong> surface (eligible + privilege
		<code>anonymous</code>): open with no session, read lobby messages only.
	</p>
	<ul>
		<li>No Auth.js session required (not under the protected-prefix list).</li>
		<li>
			GraphQL must send the application surface extension above; multi-role authed clients use
			<code>e2e-ui</code> / <code>e2e-ui-admin</code> instead.
		</li>
		<li>
			Automated proof: Rust service test
			<code>public_surface_opens_and_queries_chat_without_identity</code> (empty Session +
			chat_messages query).
		</li>
	</ul>
	<pre class="sample">{SAMPLE}</pre>
</AppPage>

<style>
	.lead {
		max-width: 42rem;
		line-height: 1.5;
		color: var(--hops-text-secondary, #444);
	}
	ul {
		max-width: 42rem;
		line-height: 1.5;
	}
	code {
		font-size: 0.9em;
	}
	.sample {
		margin-top: 1.25rem;
		padding: 1rem;
		overflow: auto;
		font-size: 0.8rem;
		background: var(--hops-bg-light, #f4f4f2);
		border: 1px solid var(--hops-border, #ddd);
		border-radius: 8px;
	}
</style>
