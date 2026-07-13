import type { DefaultSession } from '@auth/core/types';

declare module '@auth/sveltekit' {
  interface Session {
    accessToken?: string;
    idToken?: string;
    expiresAt?: number;
    error?: string;
    user?: DefaultSession['user'] & {
      id?: string;
      groups?: string[];
      username?: string;
    };
  }
}

declare global {
  namespace App {
    // interface Error {}
    // interface Locals {}
    // interface PageData {}
  }
}

export {};
