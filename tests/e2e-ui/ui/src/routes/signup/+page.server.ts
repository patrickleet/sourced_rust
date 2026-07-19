import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import {
	registerAndLogin,
	registerHuman,
	ZitadelAuthError
} from '$lib/server/zitadel-session';
import { startOidcSignIn } from '$lib/server/oidc-start';

export const load: PageServerLoad = async (event) => {
	const session = await event.locals.auth();
	if (session?.user) {
		redirect(303, '/todos');
	}

	// Optional: when coming from /login mid-OIDC, preserve authRequest to finalize without a second password entry.
	const authRequest = event.url.searchParams.get('authRequest')?.trim() ?? '';
	return { authRequest };
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const authRequest = String(form.get('authRequest') ?? '').trim();
		const username = String(form.get('username') ?? '').trim();
		const email = String(form.get('email') ?? '').trim();
		const password = String(form.get('password') ?? '');
		const givenName = String(form.get('givenName') ?? '').trim();
		const familyName = String(form.get('familyName') ?? '').trim();

		const fields = { username, email, givenName, familyName };

		if (!username || !email || !password) {
			return fail(400, {
				error: 'Username, email, and password are required.',
				...fields
			});
		}

		try {
			if (authRequest) {
				const callbackUrl = await registerAndLogin(authRequest, {
					username,
					email,
					password,
					givenName: givenName || undefined,
					familyName: familyName || undefined
				});
				redirect(303, callbackUrl);
			}

			// Cold signup: create user, then start OIDC → custom /login for password + tokens.
			await registerHuman({
				username,
				email,
				password,
				givenName: givenName || undefined,
				familyName: familyName || undefined
			});
			// Point callback at todos after Auth.js completes.
			const url = new URL(event.url);
			if (!url.searchParams.get('callbackUrl')) {
				url.searchParams.set('callbackUrl', '/todos');
			}
			await startOidcSignIn({ fetch: event.fetch, url }, 'sign-up');
		} catch (e) {
			// redirect() throws; rethrow so SvelteKit handles it
			if (e && typeof e === 'object' && 'status' in e && 'location' in e) {
				throw e;
			}
			const err = e instanceof ZitadelAuthError ? e : null;
			return fail(err?.status && err.status < 500 ? err.status : 400, {
				error: err?.message ?? 'Could not create account.',
				...fields
			});
		}
	}
};
