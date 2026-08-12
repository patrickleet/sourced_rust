import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const authCookiesPath = path.join(uiRoot, 'src/lib/server/auth-cookies.ts');
const { authCookieNamesToDelete, deleteAuthCookies } = await import(
	pathToFileURL(authCookiesPath).href
);

test('sign out deletes unchunked and chunked Auth.js cookies only', () => {
	assert.deepEqual(
		authCookieNamesToDelete([
			'authjs.callback-url',
			'authjs.session-token.0',
			'authjs.session-token.1',
			'__Secure-authjs.session-token.0',
			'__Host-authjs.csrf-token',
			'authjs.session-token.attacker',
			'other-session'
		]),
		[
			'authjs.callback-url',
			'authjs.session-token.0',
			'authjs.session-token.1',
			'__Secure-authjs.session-token.0',
			'__Host-authjs.csrf-token'
		]
	);
});

test('sign out de-duplicates cookie deletion names', () => {
	assert.deepEqual(
		authCookieNamesToDelete(['authjs.session-token', 'authjs.session-token']),
		['authjs.session-token']
	);
});

test('sign out uses HTTP-safe deletion locally and prefix-valid secure deletion', () => {
	const calls = [];
	deleteAuthCookies({
		getAll: () => [
			{ name: 'authjs.session-token.0', value: 'first' },
			{ name: 'authjs.session-token.1', value: 'second' },
			{ name: '__Secure-authjs.session-token', value: 'secure' },
			{ name: 'unrelated', value: 'keep' }
		],
		delete: (name, options) => calls.push([name, options])
	});

	assert.deepEqual(calls, [
		['authjs.session-token.0', { path: '/', secure: false }],
		['authjs.session-token.1', { path: '/', secure: false }],
		['__Secure-authjs.session-token', { path: '/', secure: true }]
	]);
});
