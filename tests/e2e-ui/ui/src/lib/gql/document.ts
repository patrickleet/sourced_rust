/**
 * Normalize GraphQL documents for the wire format.
 * Authors own `.gql` files; codegen emits TypedDocumentNode; the HTTP/WS
 * layer always sends a string query body.
 */
import { print, type DocumentNode } from 'graphql';
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';

export type GqlDocument =
	| string
	| DocumentNode
	| TypedDocumentNode<unknown, unknown>;

/** Convert string or AST document to the GraphQL source string for POST/WS. */
export function documentToString(document: GqlDocument): string {
	if (typeof document === 'string') return document;
	return print(document as DocumentNode);
}
