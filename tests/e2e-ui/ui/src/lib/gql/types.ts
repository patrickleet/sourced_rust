/** Shared GraphQL client types (browser + SSR). */

export type GqlResult<T> = {
  data?: T;
  errors?: Array<{ message: string }>;
  status: number;
};

export type GqlAuth = {
  accessToken?: string | null;
  /** DevHeaders offline fallback only — ignored under OidcBearer */
  userId?: string | null;
  role?: string | null;
};
