/**
 * Cross-demo optimism proofs: UI must paint under a held GraphQL mutation.
 *
 * These are the regression gate for "it felt slow / lost optimism". Offline
 * artifact checks live in ui/tests/optimism-artifacts.test.mjs.
 */
import { test, expect } from '@playwright/test';
import { expectOptimisticPaint } from './helpers/optimism';

const HOLD_MS = 1_500;
const ASSERT_MS = 1_000;

test.describe('demo optimism @optimism', () => {
	test('chat: post paints before the held mutation response', async ({ page }) => {
		const body = `optimism chat ${Date.now()}`;

		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /lobby/i })).toBeVisible({
			timeout: 20_000
		});

		const msg = page.locator('.ch-msg', { hasText: body });
		const response = await expectOptimisticPaint(page, {
			needle: 'chat_messages_post',
			holdMs: HOLD_MS,
			assertWithinMs: ASSERT_MS,
			act: async () => {
				await page.locator('#chat-body').fill(body);
				await page.getByRole('button', { name: /send/i }).click();
			},
			assertOptimistic: async () => {
				await expect(msg).toBeVisible({ timeout: 200 });
				await expect(msg.locator('.ch-body')).toHaveText(body);
			},
			assertConverged: async () => {
				await expect(msg).toBeVisible();
			}
		});
		expect(response.ok(), `chat_messages_post HTTP ${response.status()}`).toBeTruthy();
	});

	test('chat: full first page still paints an optimistic post', async ({ page }) => {
		/**
		 * Full offset windows used to mark the index stale instead of inserting.
		 * Seed past the live page size, then prove a held send still appears.
		 */
		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /lobby/i })).toBeVisible({
			timeout: 20_000
		});

		const log = page.locator('.ch-log');
		await expect(log).toHaveAttribute('data-chat-page-size', '25');
		const pageSize = Number(await log.getAttribute('data-chat-page-size'));

		const stamp = Date.now();
		// Ensure at least one full live page of rows exist before the held send.
		const seedCount = Math.max(pageSize, 25);
		for (let i = 0; i < seedCount; i += 1) {
			const seed = `optimism seed ${stamp} #${String(i).padStart(2, '0')}`;
			await page.locator('#chat-body').fill(seed);
			await page.getByRole('button', { name: /send/i }).click();
			await expect(page.locator('.ch-msg', { hasText: seed })).toBeVisible({
				timeout: 15_000
			});
		}

		const body = `optimism full-page ${stamp}`;
		const msg = page.locator('.ch-msg', { hasText: body });
		const response = await expectOptimisticPaint(page, {
			needle: 'chat_messages_post',
			holdMs: HOLD_MS,
			assertWithinMs: ASSERT_MS,
			act: async () => {
				await page.locator('#chat-body').fill(body);
				await page.getByRole('button', { name: /send/i }).click();
			},
			assertOptimistic: async () => {
				await expect(msg).toBeVisible({ timeout: 200 });
			}
		});
		expect(response.ok()).toBeTruthy();
		// Newest-first live window should still show the optimistic row.
		await expect(msg).toBeVisible();
	});

	test('todos: create paints in Open before the held mutation response', async ({
		page
	}) => {
		const title = `optimism todo ${Date.now()}`;

		await page.goto('/todos');
		await expect(page.getByRole('heading', { name: /todos/i })).toBeVisible({
			timeout: 20_000
		});

		const openItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });

		const response = await expectOptimisticPaint(page, {
			needle: 'todos_create',
			holdMs: HOLD_MS,
			assertWithinMs: ASSERT_MS,
			act: async () => {
				await page.locator('#todo-title').fill(title);
				await page.getByRole('button', { name: /^add$/i }).click();
			},
			assertOptimistic: async () => {
				await expect(openItem).toBeVisible({ timeout: 200 });
			},
			assertConverged: async () => {
				await expect(openItem).toBeVisible();
				expect(
					await page.locator('.board button:disabled').count(),
					'create must not leave routine controls disabled'
				).toBe(0);
			}
		});
		expect(response.ok(), `todos_create HTTP ${response.status()}`).toBeTruthy();
	});

	test('todos: complete paints in Done before the held mutation response', async ({
		page
	}) => {
		const title = `optimism complete ${Date.now()}`;

		await page.goto('/todos');
		await expect(page.getByRole('heading', { name: /todos/i })).toBeVisible({
			timeout: 20_000
		});

		// Authoritative create first so complete targets a real open row.
		await page.locator('#todo-title').fill(title);
		const createDone = page.waitForResponse(
			(r) => (r.request().postData() ?? '').includes('todos_create')
		);
		await page.getByRole('button', { name: /^add$/i }).click();
		const openItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		await expect(openItem).toBeVisible({ timeout: 20_000 });
		await createDone;

		const doneItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.item', { hasText: title });

		const response = await expectOptimisticPaint(page, {
			needle: 'todos_complete',
			holdMs: HOLD_MS,
			assertWithinMs: ASSERT_MS,
			act: async () => {
				await openItem.getByRole('button', { name: /^done$/i }).click();
			},
			assertOptimistic: async () => {
				await expect(doneItem).toBeVisible({ timeout: 200 });
			}
		});
		expect(response.ok()).toBeTruthy();
	});

	test('blob: move paints the board before the held mutation response', async ({
		page
	}) => {
		/**
		 * Same client path as todos/chat: command input fills `.applies`
		 * preview (map_json etc from pure simulate_move twin). Hold the wire
		 * and require the player cell to move before the response fulfills.
		 */
		await page.goto('/blob');
		await expect(page.locator('[data-blob-hydrated="1"]')).toBeVisible({
			timeout: 15_000
		});
		await expect(page.getByTestId('blob-start-game')).toBeEnabled({
			timeout: 10_000
		});

		const [startResp] = await Promise.all([
			page.waitForResponse(
				(r) =>
					r.url().includes('/graphql') &&
					(r.request().postData() ?? '').includes('blob_games_start'),
				{ timeout: 20_000 }
			),
			page.getByTestId('blob-start-game').click()
		]);
		expect(startResp.ok()).toBeTruthy();
		const board = page.locator('.blob-board');
		await expect(board).toBeVisible({ timeout: 15_000 });
		// Generated levels start at r0 c0.
		await expect(board.locator('.tile-player')).toHaveAttribute(
			'aria-label',
			'r0 c0'
		);

		const response = await expectOptimisticPaint(page, {
			needle: 'blob_games_move',
			holdMs: HOLD_MS,
			assertWithinMs: ASSERT_MS,
			act: async () => {
				await page.keyboard.press('ArrowRight');
			},
			assertOptimistic: async () => {
				await expect(board.locator('.tile-player')).toHaveAttribute(
					'aria-label',
					'r0 c1',
					{ timeout: 200 }
				);
			},
			assertConverged: async () => {
				await expect(board.locator('.tile-player')).toHaveAttribute(
					'aria-label',
					'r0 c1'
				);
				await expect(page.locator('.blob-empty')).toHaveCount(0);
			}
		});
		expect(response.ok(), `blob_games_move HTTP ${response.status()}`).toBeTruthy();
	});
});
