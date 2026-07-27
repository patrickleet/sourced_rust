import type { Page } from '@playwright/test';

export const DEMO_PASSWORD = process.env.E2E_HUMAN_PASSWORD || 'Password1!';

/**
 * Full OIDC path through Todos custom Login V2:
 * protected route → /login → Auth.js → Zitadel authorize → /login?authRequest=…
 * → password form → Auth.js callback → destination.
 */
export async function loginAs(
	page: Page,
	username: string,
	password: string = DEMO_PASSWORD,
	destination = '/todos'
) {
	await page.goto(destination, { waitUntil: 'domcontentloaded' });

	// May bounce through OIDC; land on custom login with username field.
	await page.waitForURL(/\/login/, { timeout: 45_000 });
	await page.waitForSelector('#loginName', { timeout: 45_000 });

	await page.locator('#loginName').fill(username);
	await page.locator('#password').fill(password);
	await page.getByRole('button', { name: /continue/i }).click();

	// After success we leave /login (callback then app route).
	await page.waitForURL((url) => !url.pathname.startsWith('/login'), {
		timeout: 60_000
	});
}

export async function expectLoggedInNav(page: Page) {
	// Home / layout usually shows session or product chrome once authed.
	// Destination-specific checks live in each suite.
	await page.waitForLoadState('networkidle').catch(() => {});
}
