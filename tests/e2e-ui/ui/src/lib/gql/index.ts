/**
 * Public GraphQL client surface for e2e-ui.
 * Prefer this barrel over deep imports when wiring routes.
 */
export type { GqlAuth, GqlResult } from './types.ts';
export type { GqlDocument } from './document.ts';
export { documentToString } from './document.ts';
export { buildAuthHeaders } from './auth-headers.ts';
export {
	wsConnectionInitPayload,
	applyWsDevHeaderParams
} from './auth-headers.ts';
export { requestGraphql } from './request.ts';
export { createGraphqlClient } from './create-client.ts';
export type { GraphqlClient, GraphqlClientOptions } from './create-client.ts';
export { defineResource } from './define-resource.ts';
export type { DefineResourceInput, GraphqlResource } from './define-resource.ts';
export { useGraphql, authFromPageData } from './use-graphql.ts';
export type { AppGraphqlClient, PageGraphqlData, UseGraphqlOptions } from './use-graphql.ts';
export { browserGraphql } from './client.ts';
// Re-export command binders for non-useGraphql call sites (tests, SSR factories).
export { bindCommands } from '$lib/api/commands.generated';
export type { BoundCommands, CommandClient } from '$lib/api/commands.generated';
export { bindCommandsPipeline } from './bind-commands-pipeline.ts';
export type {
	PipelinedBoundCommands,
	CommandCallOptions,
	CommandPolicyMap
} from './bind-commands-pipeline.ts';

/** Browser query cache + command result pipeline (ack/fact/projection). */
export {
	QueryCache,
	cacheKey,
	runCommandPipeline,
	effect,
	applyCacheOps,
	rollback
} from './cache/index.ts';
export type {
	CacheOp,
	CacheTarget,
	CommandPolicy,
	Effect,
	ResultKind,
	ReconcileKind,
	CommandPipelineOptions
} from './cache/index.ts';

/** Houdini-style document store (prefer over manual cache helpers). */
export { createDocumentStore } from './document-store.ts';
export type {
	DocumentStore,
	DocumentStoreOptions,
	DocumentStoreSnapshot,
	StoreStatus
} from './document-store.ts';

/** Escape-hatch cache helpers — prefer `gql.store` / `gql.live`. */
export {
	seedQueryCache,
	readQueryList,
	queryDocString,
	listTarget
} from './cache-helpers.ts';
