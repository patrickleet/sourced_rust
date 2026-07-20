import { test, expect } from '@playwright/test';

test.describe('unauthenticated access', () => {
	test('protected routes redirect toward login / OIDC', async ({ page }) => {
		for (const path of ['/todos', '/chat', '/blob', '/admin', '/session']) {
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

	test('home page is reachable without a session', async ({ page }) => {
		await page.goto('/');
		await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible({
			timeout: 20_000
		});
	});
});
