import { Operation_ChatMessages } from './operations/chat-messages.js';

/** GENERATED framework-neutral island inventory. */
export const DISTRIBUTED_ISLANDS = [
  {
    "version": 1,
    "id": "sha256:3850f12e756a6ad51d2ecaa1757a1e95d4d272100aa9122bf1e01d9ed28916b4",
    "operation": "ChatMessages",
    "operationHash": "sha256:f346242c36efd78f1c04d86c741b0f5c6c6f75c66d78ba9454a4647243922bfe",
    "modulePath": "operations/chat-messages.ts",
    "exportName": "Operation_ChatMessages",
    "source": {
      "path": "src/routes/chat/+layout.public.graphql",
      "line": 3,
      "column": 1
    },
    "directives": {
      "load": true,
      "live": true
    },
    "variableSchema": {
      "reference": "sha256:f346242c36efd78f1c04d86c741b0f5c6c6f75c66d78ba9454a4647243922bfe#variable-codec-v2",
      "codecVersion": 2,
      "variables": [
        {
          "name": "limit",
          "graphqlType": "Int!",
          "defaultValue": 25
        },
        {
          "name": "offset",
          "graphqlType": "Int!",
          "defaultValue": 0
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
