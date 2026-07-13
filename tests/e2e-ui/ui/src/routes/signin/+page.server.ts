import { redirect } from '@sveltejs/kit';
import type { Actions, PageServerLoad } from './$types';
import { signIn } from '../../auth';
import { isOidcConfigured } from '../../auth';

export const load: PageServerLoad = async ({ locals, url }) => {
  const session = await locals.auth();
  if (session?.user) {
    throw redirect(303, url.searchParams.get('callbackUrl') || '/todos');
  }
  return {
    oidcConfigured: isOidcConfigured(),
    callbackUrl: url.searchParams.get('callbackUrl') || '/todos',
    error: url.searchParams.get('error')
  };
};

export const actions: Actions = {
  default: async (event) => {
    const form = await event.request.formData();
    const callbackUrl = String(form.get('callbackUrl') || '/todos');
    // Auth.js sign-in to OIDC provider
    return signIn('oidc', event, { redirectTo: callbackUrl });
  }
};
