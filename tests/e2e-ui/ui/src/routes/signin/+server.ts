import type { RequestHandler } from './$types';
import { startOidcSignIn } from '$lib/server/oidc-start';

export const GET: RequestHandler = async (event) => startOidcSignIn(event, 'sign-in');
