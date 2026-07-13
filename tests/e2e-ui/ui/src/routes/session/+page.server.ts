import type { PageServerLoad } from './$types';
import { engineRoleFromGroups } from '../../auth';

export const load: PageServerLoad = async ({ locals }) => {
  const session = await locals.auth();
  const groups = session?.user?.groups ?? [];
  return {
    session,
    engineRole: engineRoleFromGroups(groups),
    hasAccessToken: Boolean(session?.accessToken)
  };
};
