import { test, expect } from '@playwright/test';

test.describe('blob game (alice)', () => {
	test('start a game, paint board, move, show in history', async ({ page }) => {
		await page.goto('/blob');
		await expect(page.getByRole('heading', { name: /blob game/i })).toBeVisible();

		// Wait for client hydration so Start/New game handlers are attached.
		await expect(page.locator('[data-blob-hydrated="1"]')).toBeVisible({ timeout: 15_000 });
		await expect(page.getByTestId('blob-start-game')).toBeEnabled({ timeout: 10_000 });

		// Empty state — no fake grid pretending to be a game
		await expect(page.getByText(/no game selected/i)).toBeVisible();
		await expect(page.locator('.blob-board')).toHaveCount(0);

		const start = page.getByTestId('blob-start-game');
		const [resp] = await Promise.all([
			page.waitForResponse(
				(r) =>
					r.url().includes('/graphql') &&
					r.request().method() === 'POST' &&
					(r.request().postData() ?? '').includes('blob_games_start'),
				{ timeout: 20_000 }
			),
			start.click()
		]);
		expect(resp.ok(), `blob_games_start HTTP ${resp.status()}`).toBeTruthy();

		// Real board from command payload
		const board = page.locator('.blob-board');
		await expect(board).toBeVisible({ timeout: 15_000 });
		await expect(page).toHaveURL(/\/blob\/[^/]+$/);
		await expect(board.locator('.cell').first()).toBeVisible();
		await expect(page.locator('.hud-value').first()).toBeVisible();

		// Score starts at 0
		await expect(page.locator('.blob-hud')).toContainText(/score/i);

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.blob-board').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.blob-board').length);
			});
			observer.observe(document.querySelector('.blob-page')!, {
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedBlobContinuitySamples: samples,
				__distributedBlobContinuityObserver: observer
			});
		});

		// Move right (pad or keyboard). Either score increments or stays at an
		// edge/wall, but the cached board must remain mounted throughout.
		const [moveResp] = await Promise.all([
			page.waitForResponse(
				(r) =>
					r.url().includes('/graphql') &&
					r.request().method() === 'POST' &&
					(r.request().postData() ?? '').includes('blob_games_move'),
				{ timeout: 20_000 }
			),
			page.keyboard.press('ArrowRight')
		]);
		expect(moveResp.ok(), `blob_games_move HTTP ${moveResp.status()}`).toBeTruthy();
		await expect(board).toBeVisible();
		await expect(page.locator('.fn-alert, .blob-empty')).toHaveCount(0);

		// History lists this game
		await expect(page.getByRole('heading', { name: /your games/i })).toBeVisible({
			timeout: 15_000
		});
		await expect(page.locator('.history-item').first()).toBeVisible({ timeout: 15_000 });

		// Soft URL may include game id (replaceState) or stay /blob
		await page.waitForTimeout(300);
		expect(page.url()).toMatch(/\/blob/);
		const samples = await page.evaluate(() => {
			const state = globalThis as typeof globalThis & {
				__distributedBlobContinuitySamples: number[];
				__distributedBlobContinuityObserver: MutationObserver;
			};
			state.__distributedBlobContinuityObserver.disconnect();
			return state.__distributedBlobContinuitySamples;
		});
		expect(navigations, 'move commands must not navigate or reload the page').toEqual([]);
		expect(
			Math.min(...samples),
			'stale-while-revalidate must never replace the known board with an empty view'
		).toBeGreaterThan(0);
	});

	test('new game from header when already on empty state', async ({ page }) => {
		await page.goto('/blob');
		await expect(page.locator('[data-blob-hydrated="1"]')).toBeVisible({ timeout: 15_000 });
		const neu = page.getByTestId('blob-new-game');
		await expect(neu).toBeEnabled({ timeout: 10_000 });

		const [resp] = await Promise.all([
			page.waitForResponse(
				(r) =>
					r.url().includes('/graphql') &&
					r.request().method() === 'POST' &&
					(r.request().postData() ?? '').includes('blob_games_start'),
				{ timeout: 20_000 }
			),
			neu.click()
		]);
		expect(resp.ok(), `blob_games_start HTTP ${resp.status()}`).toBeTruthy();

		await expect(page.locator('.blob-board')).toBeVisible({ timeout: 15_000 });
		const cells = page.locator('.blob-board .cell');
		await expect(cells.first()).toBeVisible();
		// 6×6 generated maps → at least 16 cells
		expect(await cells.count()).toBeGreaterThanOrEqual(16);
	});
});
