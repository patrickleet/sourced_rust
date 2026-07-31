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

	test('scroll-up / load-earlier fetches the next history page', async ({ page }) => {
		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /lobby/i })).toBeVisible({
			timeout: 20_000
		});

		// Seed more than one page so offset history is meaningful (page size 25).
		const stamp = Date.now();
		const total = 30;
		for (let i = 0; i < total; i += 1) {
			const body = `history seed ${stamp} #${String(i).padStart(2, '0')}`;
			await page.locator('#chat-body').fill(body);
			await page.getByRole('button', { name: /send/i }).click();
			await expect(page.locator('.ch-msg', { hasText: body })).toBeVisible({
				timeout: 15_000
			});
		}

		const log = page.locator('.ch-log');
		await expect(log).toHaveAttribute('data-chat-page-size', '25');

		// Live window is the newest 25 (#05–#29). #00 requires a history page.
		const olderThanFirstPage = `history seed ${stamp} #00`;
		const loadEarlier = page.getByTestId('chat-load-earlier');

		// If the panel did not auto-fill, explicitly load / scroll for older rows.
		if (await loadEarlier.isVisible()) {
			const before = await page.locator('.ch-msg').count();
			await loadEarlier.click();
			await expect
				.poll(async () => page.locator('.ch-msg').count(), { timeout: 20_000 })
				.toBeGreaterThan(before);
		} else {
			// Chromium column-reverse uses negative scrollTop toward the oldest edge.
			await log.evaluate((el) => {
				el.scrollTop = -(el.scrollHeight - el.clientHeight);
			});
		}

		await expect(page.locator('.ch-msg', { hasText: olderThanFirstPage })).toBeVisible({
			timeout: 20_000
		});
	});

	test('sending preserves the rendered log while revalidating', async ({ page }) => {
		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /lobby/i })).toBeVisible({
			timeout: 20_000
		});

		const baseline = `continuity baseline ${Date.now()}`;
		await page.locator('#chat-body').fill(baseline);
		const baselineResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('chat_messages_post')
		);
		await page.getByRole('button', { name: /send/i }).click();
		await expect(page.locator('.ch-msg', { hasText: baseline })).toBeVisible({
			timeout: 20_000
		});
		await baselineResponse;

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.ch-msg').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.ch-msg').length);
			});
			observer.observe(document.querySelector('.ch-log')!, {
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedChatContinuitySamples: samples,
				__distributedChatContinuityObserver: observer
			});
		});

		await page.route('**/graphql', async (route) => {
			if (!(route.request().postData() ?? '').includes('chat_messages_post')) {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			await new Promise((resolve) => setTimeout(resolve, 700));
			await route.fulfill({ response });
		});

		const body = `continuity message ${Date.now()}`;
		await page.locator('#chat-body').fill(body);
		const commandResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('chat_messages_post')
		);
		await page.getByRole('button', { name: /send/i }).click();
		await expect(page.locator('.ch-msg', { hasText: body })).toBeVisible({
			timeout: 400
		});
		await commandResponse;
		await page.unrouteAll({ behavior: 'wait' });

		const samples = await page.evaluate(() => {
			const state = globalThis as typeof globalThis & {
				__distributedChatContinuitySamples: number[];
				__distributedChatContinuityObserver: MutationObserver;
			};
			state.__distributedChatContinuityObserver.disconnect();
			return state.__distributedChatContinuitySamples;
		});
		expect(navigations, 'commands must not navigate or reload the page').toEqual([]);
		expect(
			Math.min(...samples),
			'stale-while-revalidate must never replace the known log with an empty view'
		).toBeGreaterThan(0);
	});
});
