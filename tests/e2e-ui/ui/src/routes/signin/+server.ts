/**
 * Legacy entry: start OIDC authorize. Prefer /login (custom Login V2 pages).
 * Kept so old links and docs still work.
 */
import type { RequestHandler } from './$types';
import { startOidcSignIn } from '$lib/server/oidc-start';

export const GET: RequestHandler = async (event) => startOidcSignIn(event, 'sign-in');
