import { Operation_AdminAllTodos } from './operations/admin-all-todos.js';

/** GENERATED framework-neutral island inventory. */
export const DISTRIBUTED_ISLANDS = [
  {
    "version": 1,
    "id": "sha256:817997ceaf481a314270b6f76a35b81f8f2425e581579365ab7079276141b2fe",
    "operation": "AdminAllTodos",
    "operationHash": "sha256:3436d01402748d8c4f2f5d2ff5f1173fea89786c8e902de09a24bd03596eaf27",
    "modulePath": "operations/admin-all-todos.ts",
    "exportName": "Operation_AdminAllTodos",
    "source": {
      "path": "src/routes/admin/+page.graphql",
      "line": 2,
      "column": 1
    },
    "directives": {
      "load": true,
      "live": false
    },
    "variableSchema": {
      "reference": "sha256:3436d01402748d8c4f2f5d2ff5f1173fea89786c8e902de09a24bd03596eaf27#variable-codec-v1",
      "codecVersion": 1,
      "variables": []
    },
    "liveCoverage": {
      "requested": false,
      "finite": true,
      "kind": "offset",
      "maxItems": 1000
    }
  }
] as const;

/** Static island-to-artifact bindings consumed by framework adapters. */
export const DISTRIBUTED_ISLAND_OPERATIONS = [
  { plan: DISTRIBUTED_ISLANDS[0], artifact: Operation_AdminAllTodos }
] as const;

export type DistributedIslandPlan = (typeof DISTRIBUTED_ISLANDS)[number];
export type DistributedIslandOperation = (typeof DISTRIBUTED_ISLAND_OPERATIONS)[number];
