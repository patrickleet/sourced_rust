import { Operation_AdminAllTodos } from './operations/admin-all-todos.js';

/** GENERATED framework-neutral `@load` ownership plan. */
export const DISTRIBUTED_ROUTES = [
  {
    "operation": "AdminAllTodos",
    "route": "/admin",
    "source_path": "src/routes/admin/+page.graphql",
    "discovery": "convention"
  }
] as const;

/** Static route-to-artifact bindings consumed by framework SSR adapters. */
export const DISTRIBUTED_ROUTE_OPERATIONS = [
  { plan: DISTRIBUTED_ROUTES[0], artifact: Operation_AdminAllTodos }
] as const;

export type DistributedRoutePlan = (typeof DISTRIBUTED_ROUTES)[number];
export type DistributedRouteOperation = (typeof DISTRIBUTED_ROUTE_OPERATIONS)[number];
