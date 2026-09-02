import { Operation_BlobGames } from './operations/blob-games.js';
import { Operation_ChatMessages } from './operations/chat-messages.js';
import { Operation_SelectedBlobGame } from './operations/selected-blob-game.js';
import { Operation_Todos } from './operations/todos.js';

/** GENERATED framework-neutral island inventory. */
export const DISTRIBUTED_ISLANDS = [
  {
    "version": 1,
    "id": "sha256:b2f01ff896ed296c0041116419fcf1f55d09b80cb4758bb2032e5ca623a58e8d",
    "operation": "BlobGames",
    "operationHash": "sha256:a4fa7bd07cd0202f399c6d82a9ee79e67586c5a65ee3a4098b2d84e8ef6ed4fa",
    "modulePath": "operations/blob-games.ts",
    "exportName": "Operation_BlobGames",
    "source": {
      "path": "src/routes/blob/[[gameId]]/+page.graphql",
      "line": 2,
      "column": 1
    },
    "directives": {
      "load": true,
      "live": false
    },
    "variableSchema": {
      "reference": "sha256:a4fa7bd07cd0202f399c6d82a9ee79e67586c5a65ee3a4098b2d84e8ef6ed4fa#variable-codec-v2",
      "codecVersion": 2,
      "variables": []
    },
    "liveCoverage": {
      "requested": false,
      "finite": true,
      "kind": "offset",
      "maxItems": 1000
    }
  },
  {
    "version": 1,
    "id": "sha256:121c4ce12ff5de76af6d16253f3f76b9d1a7559ec1579a1e33d7d2fd48eafb02",
    "operation": "ChatMessages",
    "operationHash": "sha256:f346242c36efd78f1c04d86c741b0f5c6c6f75c66d78ba9454a4647243922bfe",
    "modulePath": "operations/chat-messages.ts",
    "exportName": "Operation_ChatMessages",
    "source": {
      "path": "src/routes/chat/+layout.graphql",
      "line": 2,
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
  },
  {
    "version": 1,
    "id": "sha256:949ce977cf7b5e86165d7ea1f4076cd1f428f4f150585b758422040ed0670862",
    "operation": "SelectedBlobGame",
    "operationHash": "sha256:6a16ad716ee1617ca22509603ee8b07cc641076cc95d13faa79ff79d83aa8fa3",
    "modulePath": "operations/selected-blob-game.ts",
    "exportName": "Operation_SelectedBlobGame",
    "source": {
      "path": "src/lib/components/blob/SelectedBlobGame.graphql",
      "line": 3,
      "column": 1
    },
    "directives": {
      "load": true,
      "live": false
    },
    "variableSchema": {
      "reference": "sha256:6a16ad716ee1617ca22509603ee8b07cc641076cc95d13faa79ff79d83aa8fa3#variable-codec-v2",
      "codecVersion": 2,
      "variables": [
        {
          "name": "gameId",
          "graphqlType": "String"
        }
      ]
    },
    "liveCoverage": {
      "requested": false,
      "finite": true,
      "kind": "offset",
      "maxItems": 1000
    }
  },
  {
    "version": 1,
    "id": "sha256:c2c7da4e6ceb05d9fe67d19c4f9f2d37ef141f2d791523447810cd52fdf6d142",
    "operation": "Todos",
    "operationHash": "sha256:df906cbf75f2d5c11256c816a3a3726f34689e5d1a57f198484396c8c3803030",
    "modulePath": "operations/todos.ts",
    "exportName": "Operation_Todos",
    "source": {
      "path": "src/routes/todos/+page.graphql",
      "line": 2,
      "column": 1
    },
    "directives": {
      "load": true,
      "live": false
    },
    "variableSchema": {
      "reference": "sha256:df906cbf75f2d5c11256c816a3a3726f34689e5d1a57f198484396c8c3803030#variable-codec-v2",
      "codecVersion": 2,
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
  { plan: DISTRIBUTED_ISLANDS[0], artifact: Operation_BlobGames },
  { plan: DISTRIBUTED_ISLANDS[1], artifact: Operation_ChatMessages },
  { plan: DISTRIBUTED_ISLANDS[2], artifact: Operation_SelectedBlobGame },
  { plan: DISTRIBUTED_ISLANDS[3], artifact: Operation_Todos }
] as const;

export type DistributedIslandPlan = (typeof DISTRIBUTED_ISLANDS)[number];
export type DistributedIslandOperation = (typeof DISTRIBUTED_ISLAND_OPERATIONS)[number];
