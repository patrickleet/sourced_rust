/** Client helpers for subscription identity (Bearer preferred). */

export type ClientSession = {
  userId: string;
  role: string;
  accessToken?: string;
};

export function roleFromGroups(groups: string[] | undefined): 'admin' | 'user' {
  if (groups?.includes('admin')) return 'admin';
  return 'user';
}
