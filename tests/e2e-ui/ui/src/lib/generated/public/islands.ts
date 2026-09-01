import { Operation_ChatMessages } from './operations/chat-messages.js';

/** GENERATED framework-neutral island inventory. */
export const DISTRIBUTED_ISLANDS = [
  {
    "version": 1,
    "id": "sha256:227fd81589ae74dbda488a561f212ef84453875f24ece17f1dbbcd076ac84602",
    "operation": "ChatMessages",
    "operationHash": "sha256:16e82d9939a08f205efef9238393f8e4582da0e58d15ace6470d7831c0798603",
    "modulePath": "operations/chat-messages.ts",
    "exportName": "Operation_ChatMessages",
    "source": {
      "path": "src/graphql/public/chat.graphql",
      "line": 3,
      "column": 1
    },
    "directives": {
      "load": true,
      "live": true
    },
    "variableSchema": {
      "reference": "sha256:16e82d9939a08f205efef9238393f8e4582da0e58d15ace6470d7831c0798603#variable-codec-v1",
      "codecVersion": 1,
      "variables": [
        {
          "name": "limit",
          "graphqlType": "Int!"
        },
        {
          "name": "offset",
          "graphqlType": "Int!"
        }
      ]
    },
    "liveCoverage": {
      "requested": true,
      "finite": true,
      "kind": "offset",
      "maxItems": 1000
    }
  }
] as const;

/** Static island-to-artifact bindings consumed by framework adapters. */
export const DISTRIBUTED_ISLAND_OPERATIONS = [
  { plan: DISTRIBUTED_ISLANDS[0], artifact: Operation_ChatMessages }
] as const;

export type DistributedIslandPlan = (typeof DISTRIBUTED_ISLANDS)[number];
export type DistributedIslandOperation = (typeof DISTRIBUTED_ISLAND_OPERATIONS)[number];
