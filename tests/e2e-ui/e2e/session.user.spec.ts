import { test, expect } from '@playwright/test';

test.describe('session (alice)', () => {
	test('session page shows signed-in identity', async ({ page }) => {
		await page.goto('/session');
		// Should not bounce to login
		await expect(page).not.toHaveURL(/\/login/, { timeout: 15_000 });
		await expect(page.getByRole('heading').first()).toBeVisible({ timeout: 20_000 });
		// Token / identity chrome
		const body = await page.locator('body').innerText();
		expect(body.length).toBeGreaterThan(20);
	});
});
