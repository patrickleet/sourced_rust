import { Operation_BlobGames } from './operations/blob-games.js';
import { Operation_ChatMessages } from './operations/chat-messages.js';
import { Operation_Todos } from './operations/todos.js';

/** GENERATED framework-neutral `@load` ownership plan. */
export const DISTRIBUTED_ROUTES = [
  {
    "operation": "BlobGames",
    "route": "/blob/[[gameId]]",
    "source_path": "src/routes/blob/[[gameId]]/+page.graphql",
    "discovery": "convention"
  },
  {
    "operation": "ChatMessages",
    "route": "/chat",
    "source_path": "src/routes/chat/+page.graphql",
    "discovery": "convention"
  },
  {
    "operation": "Todos",
    "route": "/todos",
    "source_path": "src/routes/todos/+page.graphql",
    "discovery": "convention"
  }
] as const;

/** Static route-to-artifact bindings consumed by framework SSR adapters. */
export const DISTRIBUTED_ROUTE_OPERATIONS = [
  { plan: DISTRIBUTED_ROUTES[0], artifact: Operation_BlobGames },
  { plan: DISTRIBUTED_ROUTES[1], artifact: Operation_ChatMessages },
  { plan: DISTRIBUTED_ROUTES[2], artifact: Operation_Todos }
] as const;

export type DistributedRoutePlan = (typeof DISTRIBUTED_ROUTES)[number];
export type DistributedRouteOperation = (typeof DISTRIBUTED_ROUTE_OPERATIONS)[number];
