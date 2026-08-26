import type { Page } from '@playwright/test';

export const DEMO_PASSWORD = process.env.E2E_HUMAN_PASSWORD || 'Password1!';

/**
 * Full OIDC path through e2e-ui custom Login V2:
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
	const continueBtn = page.getByRole('button', { name: /continue/i });
	await continueBtn.waitFor({ state: 'visible', timeout: 15_000 });

	// Click and wait together. A sequential click-then-waitForURL misses a
	// fast navigation and, on a stuck Login V2 authRequest, never leaves
	// /login. One retry covers an expired authorize round-trip.
	for (let attempt = 0; attempt < 2; attempt += 1) {
		try {
			await Promise.all([
				page.waitForURL((url) => !url.pathname.startsWith('/login'), {
					timeout: 30_000
				}),
				continueBtn.click()
			]);
			return;
		} catch {
			if (attempt === 1) break;
			await page.goto(destination, { waitUntil: 'domcontentloaded' });
			await page.waitForURL(/\/login/, { timeout: 45_000 });
			await page.waitForSelector('#loginName', { timeout: 45_000 });
			await page.locator('#loginName').fill(username);
			await page.locator('#password').fill(password);
		}
	}

	throw new Error(
		`login as ${username} stayed on ${page.url()} after Continue`
	);
}

export async function expectLoggedInNav(page: Page) {
	// Home / layout usually shows session or product chrome once authed.
	// Destination-specific checks live in each suite.
	await page.waitForLoadState('networkidle').catch(() => {});
}
