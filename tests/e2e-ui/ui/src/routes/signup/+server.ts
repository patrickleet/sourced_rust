/**
 * Create-account entry.
 *
 * Requires Zitadel (make up). Bootstrap enables self-registration on the org
 * login policy so the hosted login UI shows Register.
 * Same OIDC start as /signin — after register, user lands via Auth.js callback.
 */
import type { RequestHandler } from './$types';
import { startOidcSignIn } from '$lib/server/oidc-start';

export const GET: RequestHandler = async (event) => startOidcSignIn(event, 'sign-up');
