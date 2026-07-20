import { test, expect } from '@playwright/test';

test.describe('chat (alice)', () => {
	test('post a lobby message and see it in the log', async ({ page }) => {
		const body = `e2e chat ${Date.now()}`;

		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /lobby/i })).toBeVisible({
			timeout: 20_000
		});

		await page.locator('#chat-body').fill(body);
		await page.getByRole('button', { name: /send/i }).click();

		const msg = page.locator('.ch-msg', { hasText: body });
		await expect(msg).toBeVisible({ timeout: 20_000 });
		await expect(msg.locator('.ch-body')).toHaveText(body);

		// Reload — message should still be there (RM + SSR)
		await page.reload();
		await expect(page.locator('.ch-msg', { hasText: body })).toBeVisible({
			timeout: 20_000
		});
	});
});
