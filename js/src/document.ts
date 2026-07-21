/** Normalize GraphQL documents for HTTP and WebSocket wire formats. */
import { print, type DocumentNode } from 'graphql';
import type { TypedDocumentNode } from '@graphql-typed-document-node/core';

import type { GraphqlVariables } from './types.js';

/** A source string, GraphQL AST, or code-generated typed document. */
export type GqlDocument<
	TData = unknown,
	TVariables extends GraphqlVariables = GraphqlVariables
> = string | DocumentNode | TypedDocumentNode<TData, TVariables>;

/** Convert a string or AST document to the GraphQL source sent on the wire. */
export function documentToString(document: GqlDocument): string {
	return typeof document === 'string' ? document : print(document);
}
