import { test, expect } from '@playwright/test';

test.describe('unauthenticated access', () => {
	test('protected routes redirect toward login / OIDC', async ({ page }) => {
		for (const path of ['/todos', '/blob', '/admin', '/session']) {
			await page.goto(path, { waitUntil: 'domcontentloaded' });
			// hooks → /login?callbackUrl=… → may immediately start OIDC
			await page.waitForURL(
				(url) =>
					url.pathname.includes('/login') ||
					url.pathname.includes('/signin') ||
					url.pathname.includes('/auth') ||
					url.hostname.includes('18080') ||
					url.pathname.includes('/oauth'),
				{ timeout: 45_000 }
			);
		}
	});

	test('public chat is reachable without a session', async ({ page }) => {
		await page.goto('/chat');
		await expect(page).toHaveURL(/\/chat(?:[/?#]|$)/);
		await expect(page.getByRole('heading', { name: 'Lobby' })).toBeVisible({
			timeout: 20_000
		});
	});

	test('home soft-navigation installs the anonymous chat client', async ({ page }) => {
		await page.goto('/');
		await page.waitForLoadState('networkidle');
		const continuityToken = `anonymous-chat-${Date.now()}`;
		await page.evaluate((token) => {
			Object.assign(globalThis, { __anonymousChatContinuityToken: token });
		}, continuityToken);

		await page
			.getByLabel('main navigation')
			.getByRole('link', { name: 'Chat', exact: true })
			.click();

		await expect(page).toHaveURL(/\/chat(?:[/?#]|$)/);
		await expect(page.getByRole('heading', { name: 'Lobby' })).toBeVisible({
			timeout: 20_000
		});
		expect(
			await page.evaluate(
				() =>
					(globalThis as typeof globalThis & {
						__anonymousChatContinuityToken?: string;
					}).__anonymousChatContinuityToken
			),
			'Chat navigation must preserve the current document'
		).toBe(continuityToken);
	});

	test('home page is reachable without a session', async ({ page }) => {
		await page.goto('/');
		await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible({
			timeout: 20_000
		});
	});
});
