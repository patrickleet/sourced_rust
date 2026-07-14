/**
 * Thin unified GraphQL client factory (DX pilot).
 * Inject URL + auth so SSR private env never enters the isomorphic core.
 */
import { requestGraphql } from './request';
import type { GqlAuth, GqlResult } from './types';

export type GraphqlClientOptions = {
  /** Absolute API URL or same-origin path, e.g. `/graphql` or `http://127.0.0.1:8791/graphql` */
  getUrl: () => string;
  getAuth: () => GqlAuth | Promise<GqlAuth>;
};

export type GraphqlClient = {
  request: <T = Record<string, unknown>>(
    document: string,
    variables?: Record<string, unknown>
  ) => Promise<GqlResult<T>>;
};

export function createGraphqlClient(opts: GraphqlClientOptions): GraphqlClient {
  return {
    async request<T = Record<string, unknown>>(
      document: string,
      variables: Record<string, unknown> = {}
    ): Promise<GqlResult<T>> {
      const auth = await opts.getAuth();
      return requestGraphql<T>(opts.getUrl(), document, auth, variables);
    }
  };
}
