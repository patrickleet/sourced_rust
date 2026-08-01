import { Operation_ChatMessages } from './operations/chat-messages.js';

/** GENERATED framework-neutral `@load` ownership plan. */
export const DISTRIBUTED_ROUTES = [
  {
    "operation": "ChatMessages",
    "route": "/chat",
    "source_path": "src/routes/chat/+page.graphql",
    "discovery": "convention"
  }
] as const;

/** Static route-to-artifact bindings consumed by framework SSR adapters. */
export const DISTRIBUTED_ROUTE_OPERATIONS = [
  { plan: DISTRIBUTED_ROUTES[0], artifact: Operation_ChatMessages }
] as const;

export type DistributedRoutePlan = (typeof DISTRIBUTED_ROUTES)[number];
export type DistributedRouteOperation = (typeof DISTRIBUTED_ROUTE_OPERATIONS)[number];
