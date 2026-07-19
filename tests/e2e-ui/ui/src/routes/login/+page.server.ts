import { fail, redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { loginWithPassword, ZitadelAuthError } from '$lib/server/zitadel-session';
import { startOidcSignIn } from '$lib/server/oidc-start';

export const load: PageServerLoad = async (event) => {
	const session = await event.locals.auth();
	if (session?.user) {
		redirect(303, '/todos');
	}

	const authRequest = event.url.searchParams.get('authRequest')?.trim() ?? '';
	// No pending OIDC auth request → start Auth.js authorize (redirects; lands back with authRequest).
	if (!authRequest) {
		await startOidcSignIn(event, 'sign-in');
	}

	return {
		authRequest,
		demoHint: 'Demo: alice / bob / admin · Password1!'
	};
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const authRequest = String(form.get('authRequest') ?? '').trim();
		const loginName = String(form.get('loginName') ?? '').trim();
		const password = String(form.get('password') ?? '');

		if (!authRequest) {
			return fail(400, {
				error: 'Sign-in session expired. Click Sign in again.',
				loginName
			});
		}
		if (!loginName || !password) {
			return fail(400, { error: 'Username and password are required.', loginName });
		}

		try {
			const callbackUrl = await loginWithPassword(authRequest, loginName, password);
			redirect(303, callbackUrl);
		} catch (e) {
			// SvelteKit redirect throws a Response-like object
			if (e && typeof e === 'object' && 'status' in e && 'location' in e) {
				throw e;
			}
			const err = e instanceof ZitadelAuthError ? e : null;
			return fail(err?.status && err.status < 500 ? err.status : 400, {
				error: err?.message ?? 'Sign-in failed. Check credentials and try again.',
				loginName
			});
		}
	}
};
