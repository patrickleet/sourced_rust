/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
import type * as Types from '../gql/generated/types';

import type { TypedDocumentNode as DocumentNode } from '@graphql-typed-document-node/core';
export type BlobMoveInput = {
  direction: string;
  game_id: string;
};

export type BlobStartInput = {
  game_id: string;
};

export type BlobStartLevelInput = {
  game_id: string;
};

export type ChatPostInput = {
  body: string;
  message_id: string;
  room_id: string;
};

export type TodoArchiveInput = {
  todo_id: string;
};

export type TodoCompleteInput = {
  todo_id: string;
};

export type TodoCreateInput = {
  title: string;
  todo_id: string;
};

export type TodoForceArchiveInput = {
  todo_id: string;
};

export type TodoRenameInput = {
  title: string;
  todo_id: string;
};

export type TodoReopenInput = {
  todo_id: string;
};

export type Command_Todos_CreateMutationVariables = Exact<{
  input: Types.TodoCreateInput;
}>;


export type Command_Todos_CreateMutation = { todos_create: { todo_id: string, owner_id: string, title: string, status: string } };

export type Command_Todos_CompleteMutationVariables = Exact<{
  input: Types.TodoCompleteInput;
}>;


export type Command_Todos_CompleteMutation = { todos_complete: { todo_id: string, status: string } };

export type Command_Todos_ArchiveMutationVariables = Exact<{
  input: Types.TodoArchiveInput;
}>;


export type Command_Todos_ArchiveMutation = { todos_archive: { todo_id: string, status: string } };

export type Command_Todos_Force_ArchiveMutationVariables = Exact<{
  input: Types.TodoForceArchiveInput;
}>;


export type Command_Todos_Force_ArchiveMutation = { todos_force_archive: { todo_id: string, owner_id: string, status: string, archived_by: string } };

export type Command_Todos_RenameMutationVariables = Exact<{
  input: Types.TodoRenameInput;
}>;


export type Command_Todos_RenameMutation = { todos_rename: { todo_id: string, title: string, status: string } };

export type Command_Todos_ReopenMutationVariables = Exact<{
  input: Types.TodoReopenInput;
}>;


export type Command_Todos_ReopenMutation = { todos_reopen: { todo_id: string, status: string } };

export type Command_Chat_Messages_PostMutationVariables = Exact<{
  input: Types.ChatPostInput;
}>;


export type Command_Chat_Messages_PostMutation = { chat_messages_post: { message_id: string, room_id: string, author_id: string, body: string, created_at: string } };

export type Command_Blob_Games_StartMutationVariables = Exact<{
  input: Types.BlobStartInput;
}>;


export type Command_Blob_Games_StartMutation = { blob_games_start: { game_id: string, owner_id: string, score: unknown, player_dead: boolean, current_level: unknown, current_level_completed: boolean, map_json: string, status: string } };

export type Command_Blob_Games_MoveMutationVariables = Exact<{
  input: Types.BlobMoveInput;
}>;


export type Command_Blob_Games_MoveMutation = { blob_games_move: { game_id: string, owner_id: string, score: unknown, player_dead: boolean, current_level: unknown, current_level_completed: boolean, map_json: string, status: string } };

export type Command_Blob_Games_Start_LevelMutationVariables = Exact<{
  input: Types.BlobStartLevelInput;
}>;


export type Command_Blob_Games_Start_LevelMutation = { blob_games_start_level: { game_id: string, owner_id: string, score: unknown, player_dead: boolean, current_level: unknown, current_level_completed: boolean, map_json: string, status: string } };


export const Command_Todos_CreateDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_todos_create"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"TodoCreateInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_create"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"title"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Todos_CreateMutation, Command_Todos_CreateMutationVariables>;
export const Command_Todos_CompleteDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_todos_complete"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"TodoCompleteInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_complete"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Todos_CompleteMutation, Command_Todos_CompleteMutationVariables>;
export const Command_Todos_ArchiveDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_todos_archive"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"TodoArchiveInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_archive"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Todos_ArchiveMutation, Command_Todos_ArchiveMutationVariables>;
export const Command_Todos_Force_ArchiveDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_todos_force_archive"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"TodoForceArchiveInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_force_archive"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"status"}},{"kind":"Field","name":{"kind":"Name","value":"archived_by"}}]}}]}}]} as unknown as DocumentNode<Command_Todos_Force_ArchiveMutation, Command_Todos_Force_ArchiveMutationVariables>;
export const Command_Todos_RenameDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_todos_rename"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"TodoRenameInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_rename"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"title"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Todos_RenameMutation, Command_Todos_RenameMutationVariables>;
export const Command_Todos_ReopenDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_todos_reopen"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"TodoReopenInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_reopen"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Todos_ReopenMutation, Command_Todos_ReopenMutationVariables>;
export const Command_Chat_Messages_PostDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_chat_messages_post"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"ChatPostInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"chat_messages_post"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"message_id"}},{"kind":"Field","name":{"kind":"Name","value":"room_id"}},{"kind":"Field","name":{"kind":"Name","value":"author_id"}},{"kind":"Field","name":{"kind":"Name","value":"body"}},{"kind":"Field","name":{"kind":"Name","value":"created_at"}}]}}]}}]} as unknown as DocumentNode<Command_Chat_Messages_PostMutation, Command_Chat_Messages_PostMutationVariables>;
export const Command_Blob_Games_StartDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_blob_games_start"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"BlobStartInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"blob_games_start"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"game_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"score"}},{"kind":"Field","name":{"kind":"Name","value":"player_dead"}},{"kind":"Field","name":{"kind":"Name","value":"current_level"}},{"kind":"Field","name":{"kind":"Name","value":"current_level_completed"}},{"kind":"Field","name":{"kind":"Name","value":"map_json"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Blob_Games_StartMutation, Command_Blob_Games_StartMutationVariables>;
export const Command_Blob_Games_MoveDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_blob_games_move"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"BlobMoveInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"blob_games_move"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"game_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"score"}},{"kind":"Field","name":{"kind":"Name","value":"player_dead"}},{"kind":"Field","name":{"kind":"Name","value":"current_level"}},{"kind":"Field","name":{"kind":"Name","value":"current_level_completed"}},{"kind":"Field","name":{"kind":"Name","value":"map_json"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Blob_Games_MoveMutation, Command_Blob_Games_MoveMutationVariables>;
export const Command_Blob_Games_Start_LevelDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"Command_blob_games_start_level"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"input"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"BlobStartLevelInput"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"blob_games_start_level"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"Variable","name":{"kind":"Name","value":"input"}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"game_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"score"}},{"kind":"Field","name":{"kind":"Name","value":"player_dead"}},{"kind":"Field","name":{"kind":"Name","value":"current_level"}},{"kind":"Field","name":{"kind":"Name","value":"current_level_completed"}},{"kind":"Field","name":{"kind":"Name","value":"map_json"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<Command_Blob_Games_Start_LevelMutation, Command_Blob_Games_Start_LevelMutationVariables>;