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
