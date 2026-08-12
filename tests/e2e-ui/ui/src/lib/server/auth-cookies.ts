const AUTH_COOKIE_BASE_NAMES = Object.freeze([
	'authjs.session-token',
	'authjs.callback-url',
	'authjs.csrf-token',
	'__Secure-authjs.session-token',
	'__Secure-authjs.callback-url',
	'__Secure-authjs.csrf-token',
	'__Host-authjs.csrf-token'
]);

function isAuthCookie(name: string): boolean {
	return AUTH_COOKIE_BASE_NAMES.some((base) => {
		if (name === base) return true;
		const suffix = name.slice(base.length);
		return name.startsWith(base) && /^\.\d+$/.test(suffix);
	});
}

/** Return only Auth.js cookies, including chunked `.<index>` JWT cookies. */
export function authCookieNamesToDelete(names: readonly string[]): string[] {
	return [...new Set(names.filter(isAuthCookie))];
}

/** Delete the current request's Auth.js cookies with prefix-valid security. */
export function deleteAuthCookies(cookies: Pick<Cookies, 'getAll' | 'delete'>): void {
	for (const name of authCookieNamesToDelete(cookies.getAll().map(({ name }) => name))) {
		const secure = name.startsWith('__Secure-') || name.startsWith('__Host-');
		cookies.delete(name, { path: '/', secure });
	}
}
import type { Cookies } from '@sveltejs/kit';
