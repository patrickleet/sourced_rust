import { test, expect } from '@playwright/test';

async function visibleTodoOrders(page: import('@playwright/test').Page) {
	return page.locator('.fn-list').evaluateAll((lists) =>
		lists.map((list) =>
			[...list.querySelectorAll<HTMLElement>('[data-todo-id]')].map(
				(item) => item.dataset.todoId ?? ''
			)
		)
	);
}

function expectBinarySorted(orders: string[][]) {
	for (const ids of orders) {
		expect(ids, `todo ids must stay in generated binary order: ${ids.join(', ')}`).toEqual(
			[...ids].sort()
		);
	}
}

test.describe('todos (alice)', () => {
	test('create, complete, reopen, archive', async ({ page }) => {
		const title = `e2e todo ${Date.now()}`;

		await page.goto('/todos');
		await expect(page.getByRole('heading', { name: /field notes/i })).toBeVisible();

		// Create
		await page.locator('#todo-title').fill(title);
		await page.getByRole('button', { name: /^add$/i }).click();
		const openItem = page
			.locator('.fn-panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.fn-item', { hasText: title });
		await expect(openItem).toBeVisible({ timeout: 15_000 });

		// Complete
		await openItem.getByRole('button', { name: /^done$/i }).click();
		const doneItem = page
			.locator('.fn-panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.fn-item', { hasText: title });
		await expect(doneItem).toBeVisible({ timeout: 15_000 });

		// Reopen (prefer the text button; wait until not busy)
		const reopen = doneItem.getByRole('button', { name: 'Reopen', exact: true });
		await expect(reopen).toBeEnabled({ timeout: 10_000 });
		await reopen.click();
		const openAgain = page
			.locator('.fn-panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.fn-item', { hasText: title });
		await expect(openAgain).toBeVisible({ timeout: 15_000 });

		// Archive
		const archive = openAgain.getByRole('button', { name: 'Archive', exact: true });
		await expect(archive).toBeEnabled({ timeout: 10_000 });
		await archive.click();
		const archiveDetails = page.locator('details.fn-archive');
		await expect(archiveDetails).toBeVisible({ timeout: 15_000 });
		await archiveDetails.locator('summary').click();
		await expect(archiveDetails.locator('.fn-item', { hasText: title })).toBeVisible({
			timeout: 10_000
		});

		// Persist across navigation (async projectors may lag a beat)
		await expect
			.poll(
				async () => {
					await page.goto('/todos');
					const again = page.locator('details.fn-archive');
					if (!(await again.isVisible().catch(() => false))) return false;
					await again.locator('summary').click();
					return again.locator('.fn-item', { hasText: title }).isVisible();
				},
				{ timeout: 25_000 }
			)
			.toBeTruthy();
	});

	test('empty title cannot submit', async ({ page }) => {
		await page.goto('/todos');
		const add = page.getByRole('button', { name: /^add$/i });
		await expect(add).toBeDisabled();
	});

	test('commands preserve rendered cache while revalidating', async ({ page }) => {
		await page.goto('/todos');
		await expect(page.getByRole('heading', { name: /field notes/i })).toBeVisible();

		// Establish one row that must remain visible while later commands update.
		const baseline = `continuity baseline ${Date.now()}`;
		await page.locator('#todo-title').fill(baseline);
		await page.getByRole('button', { name: /^add$/i }).click();
		await expect(page.locator('.fn-item', { hasText: baseline })).toBeVisible();

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.fn-item').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.fn-item').length);
			});
			observer.observe(document.querySelector('.fn-board')!, {
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedContinuitySamples: samples,
				__distributedContinuityObserver: observer
			});
		});

		const changed = `continuity changed ${Date.now()}`;
		await page.locator('#todo-title').fill(changed);
		await page.getByRole('button', { name: /^add$/i }).click();
		const openItem = page
			.locator('.fn-panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.fn-item', { hasText: changed });
		await expect(openItem).toBeVisible();
		await openItem.getByRole('button', { name: /^done$/i }).click();
		await expect(
			page
				.locator('.fn-panel')
				.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
				.locator('.fn-item', { hasText: changed })
		).toBeVisible();

		const samples = await page.evaluate(() => {
			const state = globalThis as typeof globalThis & {
				__distributedContinuitySamples: number[];
				__distributedContinuityObserver: MutationObserver;
			};
			state.__distributedContinuityObserver.disconnect();
			return state.__distributedContinuitySamples;
		});
		expect(navigations, 'commands must not navigate or reload the page').toEqual([]);
		expect(
			Math.min(...samples),
			'stale-while-revalidate must never replace known rows with an empty view'
		).toBeGreaterThan(0);
	});

	test('optimistic and authoritative states keep the same generated order', async ({
		page
	}) => {
		await page.goto('/todos');
		await expect(page.getByRole('heading', { name: /field notes/i })).toBeVisible();

		await page.route('**/graphql', async (route) => {
			const body = route.request().postData() ?? '';
			if (!body.includes('todos_create') && !body.includes('todos_complete')) {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			await new Promise((resolve) => setTimeout(resolve, 700));
			await route.fulfill({ response });
		});

		const title = `ordered optimism ${Date.now()}`;
		await page.locator('#todo-title').fill(title);
		const createResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('todos_create')
		);
		await page.getByRole('button', { name: /^add$/i }).click();
		const openItem = page
			.locator('.fn-panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.fn-item', { hasText: title });
		await expect(openItem).toBeVisible({ timeout: 400 });
		expectBinarySorted(await visibleTodoOrders(page));
		await createResponse;
		await expect(openItem).toBeVisible();
		expectBinarySorted(await visibleTodoOrders(page));

		const completeResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('todos_complete')
		);
		await openItem.getByRole('button', { name: /^done$/i }).click();
		const doneItem = page
			.locator('.fn-panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.fn-item', { hasText: title });
		await expect(doneItem).toBeVisible({ timeout: 400 });
		expectBinarySorted(await visibleTodoOrders(page));
		await completeResponse;
		await expect(doneItem).toBeVisible();
		expectBinarySorted(await visibleTodoOrders(page));
		await page.unrouteAll({ behavior: 'wait' });
	});
});
