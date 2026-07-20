/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
import type * as Types from '../../lib/gql/generated/types';

import type { TypedDocumentNode as DocumentNode } from '@graphql-typed-document-node/core';
export type BlobGamesQueryVariables = Exact<{ [key: string]: never; }>;


export type BlobGamesQuery = { blob_games: Array<{ game_id: string, owner_id: string, score: unknown, player_dead: boolean, current_level: unknown, current_level_completed: boolean, map_json: string, status: string, owner: { user_id: string, display_name: string, email: string, status: string } | null }> };


export const BlobGamesDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"BlobGames"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"blob_games"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"game_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"score"}},{"kind":"Field","name":{"kind":"Name","value":"player_dead"}},{"kind":"Field","name":{"kind":"Name","value":"current_level"}},{"kind":"Field","name":{"kind":"Name","value":"current_level_completed"}},{"kind":"Field","name":{"kind":"Name","value":"map_json"}},{"kind":"Field","name":{"kind":"Name","value":"status"}},{"kind":"Field","name":{"kind":"Name","value":"owner"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"user_id"}},{"kind":"Field","name":{"kind":"Name","value":"display_name"}},{"kind":"Field","name":{"kind":"Name","value":"email"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]}}]} as unknown as DocumentNode<BlobGamesQuery, BlobGamesQueryVariables>;