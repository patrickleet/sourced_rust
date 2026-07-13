import { signOut } from '../../../auth';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = async (event) => {
  return signOut(event, { redirectTo: '/' });
};

export const POST: RequestHandler = async (event) => {
  return signOut(event, { redirectTo: '/' });
};
