import { test, expect } from '@playwright/test';
import { expectOptimisticPaint } from './helpers/optimism';

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
		test.setTimeout(180_000);
		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /lobby/i })).toBeVisible({
			timeout: 20_000
		});

		const send = page.getByRole('button', { name: /send/i });
		// Seed more than one page so offset history is meaningful (page size 25).
		// Wait for Send to re-enable after Eventual `projected` — a click while
		// busy is ignored and the 90s test timeout then fires on locator.click.
		const stamp = Date.now();
		const total = 30;
		for (let i = 0; i < total; i += 1) {
			const body = `history seed ${stamp} #${String(i).padStart(2, '0')}`;
			await page.locator('#chat-body').fill(body);
			await send.click();
			await expect(page.locator('.ch-msg', { hasText: body })).toBeVisible({
				timeout: 15_000
			});
			// Eventual `projected` holds `busy`; the next fill would enable
			// Send while click is still ignored.
			await expect(send).toBeEnabled({ timeout: 30_000 });
		}

		const log = page.locator('.ch-log');
		await expect(log).toHaveAttribute('data-chat-page-size', '25');
		// Live window holds the newest 25; wait until at least that many render.
		await expect
			.poll(async () => page.locator('.ch-msg').count(), { timeout: 20_000 })
			.toBeGreaterThanOrEqual(25);

		// A full first page must still accept an optimistic local insert. This
		// used to live in a second scenario that seeded the same 25 rows again.
		const fullPageBody = `history optimism ${stamp}`;
		const fullPageMessage = page.locator('.ch-msg', { hasText: fullPageBody });
		const fullPageResponse = await expectOptimisticPaint(page, {
			needle: 'chat_messages_post',
			holdMs: 1_500,
			assertWithinMs: 300,
			act: async () => {
				await page.locator('#chat-body').fill(fullPageBody);
				await page.getByRole('button', { name: /send/i }).click();
			},
			assertOptimistic: async () => {
				await expect(fullPageMessage).toBeVisible({ timeout: 200 });
			},
			assertConverged: async () => {
				await expect(fullPageMessage).toBeVisible();
			}
		});
		expect(fullPageResponse.ok()).toBeTruthy();

		// Live window is the newest 25 (#05–#29). #00 requires a history page.
		const olderThanFirstPage = `history seed ${stamp} #00`;

		// Drive history via the product path (scroll to oldest edge under
		// column-reverse). The load-earlier control sits at the visual top and
		// is often off-screen while stick-to-bottom is active, so use force
		// click as a fallback rather than Playwright's auto-scroll-into-view
		// (which fights column-reverse and can leave the button detached).
		await expect
			.poll(
				async () => {
					if (
						(await page.locator('.ch-msg', { hasText: olderThanFirstPage }).count()) > 0
					) {
						return true;
					}
					await log.evaluate((el) => {
						const range = Math.max(0, el.scrollHeight - el.clientHeight);
						// Chromium column-reverse uses negative scrollTop toward oldest.
						el.scrollTop = range > 0 ? -range : 0;
						el.dispatchEvent(new Event('scroll', { bubbles: true }));
					});
					const loadEarlier = page.getByTestId('chat-load-earlier');
					if ((await loadEarlier.count()) > 0) {
						await loadEarlier.click({ force: true });
					}
					return false;
				},
				{ timeout: 45_000 }
			)
			.toBe(true);

		await expect(page.locator('.ch-msg', { hasText: olderThanFirstPage })).toBeVisible({
			timeout: 5_000
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

		const body = `continuity message ${Date.now()}`;
		const msg = page.locator('.ch-msg', { hasText: body });
		await expectOptimisticPaint(page, {
			needle: 'chat_messages_post',
			holdMs: 1_500,
			assertWithinMs: 300,
			act: async () => {
				await page.locator('#chat-body').fill(body);
				await page.getByRole('button', { name: /send/i }).click();
			},
			assertOptimistic: async () => {
				await expect(msg).toBeVisible({ timeout: 200 });
			}
		});

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
