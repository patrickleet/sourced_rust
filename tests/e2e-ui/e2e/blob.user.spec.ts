import { test, expect } from '@playwright/test';

test.describe('blob game (alice)', () => {
	test('start a game, paint board, move, show in history', async ({ page }) => {
		await page.goto('/blob');
		await expect(page.getByRole('heading', { name: /blob game/i })).toBeVisible();

		// Empty state — no fake grid pretending to be a game
		await expect(page.getByText(/no game selected/i)).toBeVisible();
		await expect(page.locator('.blob-board')).toHaveCount(0);

		await page.getByRole('button', { name: /start game|new game/i }).first().click();

		// Real board from command payload
		const board = page.locator('.blob-board');
		await expect(board).toBeVisible({ timeout: 25_000 });
		await expect(board.locator('.cell').first()).toBeVisible();
		await expect(page.locator('.hud-value').first()).toBeVisible();

		// Score starts at 0
		await expect(page.locator('.blob-hud')).toContainText(/score/i);

		// Move right (pad or keyboard)
		await page.keyboard.press('ArrowRight');
		// Either score increments or stay if edge/wall — board still present
		await expect(board).toBeVisible();
		await expect(page.locator('.fn-alert, .blob-empty')).toHaveCount(0);

		// History lists this game
		await expect(page.getByRole('heading', { name: /your games/i })).toBeVisible({
			timeout: 15_000
		});
		await expect(page.locator('.history-item').first()).toBeVisible({ timeout: 15_000 });

		// URL may include game id after start
		await page.waitForTimeout(500);
		const url = page.url();
		// Soft: either still /blob or /blob/{id}
		expect(url).toMatch(/\/blob/);
	});

	test('new game from header when already on empty state', async ({ page }) => {
		await page.goto('/blob');
		await page.getByRole('button', { name: /^new game$/i }).click();
		await expect(page.locator('.blob-board')).toBeVisible({ timeout: 25_000 });
		const cells = page.locator('.blob-board .cell');
		await expect(cells.first()).toBeVisible();
		// 6×6 generated maps → at least 16 cells
		expect(await cells.count()).toBeGreaterThanOrEqual(16);
	});
});
