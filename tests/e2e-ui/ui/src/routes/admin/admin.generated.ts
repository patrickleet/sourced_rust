/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
import * as Types from '../../lib/gql/generated/types';

import { TypedDocumentNode as DocumentNode } from '@graphql-typed-document-node/core';
export type AdminAllTodosQueryVariables = Exact<{ [key: string]: never; }>;


export type AdminAllTodosQuery = { todos: Array<{ todo_id: string, owner_id: string, title: string, status: string }> };

export type AdminForceArchiveMutationVariables = Exact<{
  todo_id: string;
}>;


export type AdminForceArchiveMutation = { todos_force_archive: { todo_id: string, owner_id: string, status: string, archived_by: string } };


export const AdminAllTodosDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"query","name":{"kind":"Name","value":"AdminAllTodos"},"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"limit"},"value":{"kind":"IntValue","value":"100"}},{"kind":"Argument","name":{"kind":"Name","value":"order_by"},"value":{"kind":"ListValue","values":[{"kind":"ObjectValue","fields":[{"kind":"ObjectField","name":{"kind":"Name","value":"todo_id"},"value":{"kind":"EnumValue","value":"asc"}}]}]}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"title"}},{"kind":"Field","name":{"kind":"Name","value":"status"}}]}}]}}]} as unknown as DocumentNode<AdminAllTodosQuery, AdminAllTodosQueryVariables>;
export const AdminForceArchiveDocument = {"kind":"Document","definitions":[{"kind":"OperationDefinition","operation":"mutation","name":{"kind":"Name","value":"AdminForceArchive"},"variableDefinitions":[{"kind":"VariableDefinition","variable":{"kind":"Variable","name":{"kind":"Name","value":"todo_id"}},"type":{"kind":"NonNullType","type":{"kind":"NamedType","name":{"kind":"Name","value":"String"}}}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todos_force_archive"},"arguments":[{"kind":"Argument","name":{"kind":"Name","value":"input"},"value":{"kind":"ObjectValue","fields":[{"kind":"ObjectField","name":{"kind":"Name","value":"todo_id"},"value":{"kind":"Variable","name":{"kind":"Name","value":"todo_id"}}}]}}],"selectionSet":{"kind":"SelectionSet","selections":[{"kind":"Field","name":{"kind":"Name","value":"todo_id"}},{"kind":"Field","name":{"kind":"Name","value":"owner_id"}},{"kind":"Field","name":{"kind":"Name","value":"status"}},{"kind":"Field","name":{"kind":"Name","value":"archived_by"}}]}}]}}]} as unknown as DocumentNode<AdminForceArchiveMutation, AdminForceArchiveMutationVariables>;