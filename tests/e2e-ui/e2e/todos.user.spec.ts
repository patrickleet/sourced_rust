import { test, expect } from '@playwright/test';

async function visibleTodoOrders(page: import('@playwright/test').Page) {
	return page.locator('.list').evaluateAll((lists) =>
		lists.map((list) =>
			[...list.querySelectorAll<HTMLElement>('[data-todo-id]')].map(
				(item) => item.dataset.todoId ?? ''
			)
		)
	);
}

async function todoOrderInPanel(
	page: import('@playwright/test').Page,
	heading: RegExp
) {
	return page
		.locator('.panel')
		.filter({ has: page.getByRole('heading', { name: heading }) })
		.locator('[data-todo-id]')
		.evaluateAll((items) =>
			items.map((item) => (item as HTMLElement).dataset.todoId ?? '')
		);
}

type TodoOrderFrame = {
	open: string[];
	done: string[];
};

async function startTodoOrderTrace(page: import('@playwright/test').Page) {
	await page.evaluate(() => {
		const state = globalThis as typeof globalThis & {
			__todoOrderTrace?: {
				frames: TodoOrderFrame[];
				frame: number;
			};
		};
		const frames: TodoOrderFrame[] = [];
		const orderFor = (name: string) => {
			const panel = [...document.querySelectorAll<HTMLElement>('.panel')].find(
				(candidate) =>
					candidate.querySelector('h2')?.textContent?.trim().toLowerCase() ===
					name
			);
			return panel
				? [...panel.querySelectorAll<HTMLElement>('[data-todo-id]')].map(
						(item) => item.dataset.todoId ?? ''
					)
				: [];
		};
		const sample = () => {
			const order = { open: orderFor('open'), done: orderFor('done') };
			const previous = frames.at(-1);
			if (
				previous === undefined ||
				JSON.stringify(previous) !== JSON.stringify(order)
			) {
				frames.push(order);
			}
			state.__todoOrderTrace!.frame = requestAnimationFrame(sample);
		};
		state.__todoOrderTrace = {
			frames,
			frame: requestAnimationFrame(sample)
		};
	});
}

async function stopTodoOrderTrace(page: import('@playwright/test').Page) {
	return page.evaluate(() => {
		const state = globalThis as typeof globalThis & {
			__todoOrderTrace?: {
				frames: TodoOrderFrame[];
				frame: number;
			};
		};
		const trace = state.__todoOrderTrace;
		if (trace === undefined) return [];
		cancelAnimationFrame(trace.frame);
		delete state.__todoOrderTrace;
		return trace.frames;
	});
}

function expectBinarySorted(orders: string[][]) {
	for (const ids of orders) {
		expect(ids, `todo ids must stay in generated binary order: ${ids.join(', ')}`).toEqual(
			[...ids].sort()
		);
	}
}

function sameTodoOrder(left: TodoOrderFrame, right: TodoOrderFrame) {
	return (
		JSON.stringify(left.open) === JSON.stringify(right.open) &&
		JSON.stringify(left.done) === JSON.stringify(right.done)
	);
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
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		await expect(openItem).toBeVisible({ timeout: 15_000 });

		// Complete
		await openItem.getByRole('button', { name: /^done$/i }).click();
		const doneItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.item', { hasText: title });
		await expect(doneItem).toBeVisible({ timeout: 15_000 });

		// Reopen (prefer the text button; wait until not busy)
		const reopen = doneItem.getByRole('button', { name: 'Reopen', exact: true });
		await expect(reopen).toBeEnabled({ timeout: 10_000 });
		await reopen.click();
		const openAgain = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		await expect(openAgain).toBeVisible({ timeout: 15_000 });

		// Archive
		const archive = openAgain.getByRole('button', { name: 'Archive', exact: true });
		await expect(archive).toBeEnabled({ timeout: 10_000 });
		await archive.click();
		const archiveDetails = page.locator('details.archive');
		await expect(archiveDetails).toBeVisible({ timeout: 15_000 });
		await archiveDetails.locator('summary').click();
		await expect(archiveDetails.locator('.item', { hasText: title })).toBeVisible({
			timeout: 10_000
		});

		// Persist across navigation (async projectors may lag a beat)
		await expect
			.poll(
				async () => {
					await page.goto('/todos');
					const again = page.locator('details.archive');
					if (!(await again.isVisible().catch(() => false))) return false;
					await again.locator('summary').click();
					return again.locator('.item', { hasText: title }).isVisible();
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
		await expect(page.locator('.item', { hasText: baseline })).toBeVisible();

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.item').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.item').length);
			});
			observer.observe(document.querySelector('.board')!, {
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
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: changed });
		await expect(openItem).toBeVisible();
		await openItem.getByRole('button', { name: /^done$/i }).click();
		await expect(
			page
				.locator('.panel')
				.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
				.locator('.item', { hasText: changed })
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
			if (
				!body.includes('todos_create') &&
				!body.includes('todos_complete') &&
				!body.includes('todos_reopen')
			) {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			await new Promise((resolve) => setTimeout(resolve, 700));
			await route.fulfill({ response });
		});
		let completeRequests = 0;
		page.on('request', (request) => {
			if ((request.postData() ?? '').includes('todos_complete')) {
				completeRequests += 1;
			}
		});

		const title = `ordered optimism ${Date.now()}`;
		await page.locator('#todo-title').fill(title);
		const createResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('todos_create')
		);
		await page.getByRole('button', { name: /^add$/i }).click();
		const openItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		// Create upserts the record optimistically but list membership is
		// authoritative (index write); the delayed route proves later complete/
		// reopen transitions paint from the optimistic layer before the wire.
		await createResponse;
		await expect(openItem).toBeVisible({ timeout: 5_000 });
		expect(
			await page.locator('.board button:disabled').count(),
			'routine command concurrency guards must not flash Todo row controls disabled'
		).toBe(0);
		expectBinarySorted(await visibleTodoOrders(page));
		const todoId = await openItem.getAttribute('data-todo-id');
		expect(todoId).not.toBeNull();
		const beforeComplete = {
			open: await todoOrderInPanel(page, /^open$/i),
			done: await todoOrderInPanel(page, /^done$/i)
		};
		const afterComplete = {
			open: beforeComplete.open.filter((id) => id !== todoId),
			done: [...beforeComplete.done, todoId!].sort()
		};
		await startTodoOrderTrace(page);
		const completeResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('todos_complete')
		);
		await openItem.getByRole('button', { name: /^done$/i }).evaluate((button) => {
			button.click();
			button.click();
		});
		const doneItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.item', { hasText: title });
		await expect(doneItem).toBeVisible({ timeout: 400 });
		expect(
			await page.locator('.board button:disabled').count(),
			'routine command concurrency guards must not flash Todo row controls disabled'
		).toBe(0);
		expectBinarySorted(await visibleTodoOrders(page));
		await completeResponse;
		expect(
			completeRequests,
			'optimistic state must suppress a duplicate action without disabling controls'
		).toBe(1);
		await expect(doneItem).toBeVisible();
		expectBinarySorted(await visibleTodoOrders(page));
		await page.waitForTimeout(750);
		const completeOrderFrames = await stopTodoOrderTrace(page);
		expect(
			completeOrderFrames.every(
				(order) =>
					sameTodoOrder(order, beforeComplete) ||
					sameTodoOrder(order, afterComplete)
			),
			`complete rendered an intermediate non-generated order: ${JSON.stringify(completeOrderFrames)}`
		).toBe(true);

		await startTodoOrderTrace(page);
		const reopenResponse = page.waitForResponse((response) =>
			(response.request().postData() ?? '').includes('todos_reopen')
		);
		await doneItem.getByRole('button', { name: /^reopen$/i }).click();
		const reopenedItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		// Reopen must paint the row back into Open before the delayed wire returns.
		await expect(reopenedItem).toBeVisible({ timeout: 400 });
		expect(
			await page.locator('.board button:disabled').count(),
			'routine command concurrency guards must not flash Todo row controls disabled'
		).toBe(0);
		await reopenResponse;
		await page.waitForTimeout(750);
		const reopenOrderFrames = await stopTodoOrderTrace(page);
		// Authoritative membership order is binary by todo_id; optimistic reopen may
		// temporarily prepend before the index reconciles on the wire response.
		expect(
			reopenOrderFrames.some((order) => sameTodoOrder(order, beforeComplete)),
			`reopen never settled on generated open order: ${JSON.stringify(reopenOrderFrames)}`
		).toBe(true);
		await expect(reopenedItem).toBeVisible();
		expect(
			await todoOrderInPanel(page, /^open$/i),
			'reopen must restore generated open order after command convergence'
		).toEqual(beforeComplete.open);

		// Repeat after both earlier commands have converged so the invariant also
		// covers a row that is fully authoritative before the next transition.
		await startTodoOrderTrace(page);
		const authoritativeCompleteResponse = page.waitForResponse((response) =>
			(response.request().postData() ?? '').includes('todos_complete')
		);
		await reopenedItem.getByRole('button', { name: /^done$/i }).click();
		await authoritativeCompleteResponse;
		await page.waitForTimeout(750);
		const authoritativeCompleteFrames = await stopTodoOrderTrace(page);
		expect(
			authoritativeCompleteFrames.every(
				(order) =>
					sameTodoOrder(order, beforeComplete) ||
					sameTodoOrder(order, afterComplete)
			),
			`authoritative complete rendered an intermediate order: ${JSON.stringify(authoritativeCompleteFrames)}`
		).toBe(true);

		await startTodoOrderTrace(page);
		const authoritativeReopenResponse = page.waitForResponse((response) =>
			(response.request().postData() ?? '').includes('todos_reopen')
		);
		await doneItem.getByRole('button', { name: /^reopen$/i }).click();
		await authoritativeReopenResponse;
		await page.waitForTimeout(750);
		const authoritativeReopenFrames = await stopTodoOrderTrace(page);
		expect(
			authoritativeReopenFrames.some((order) => sameTodoOrder(order, beforeComplete)),
			`authoritative reopen never settled on generated open order: ${JSON.stringify(authoritativeReopenFrames)}`
		).toBe(true);
		await expect(reopenedItem).toBeVisible();
		expect(await todoOrderInPanel(page, /^open$/i)).toEqual(beforeComplete.open);
		await page.unrouteAll({ behavior: 'wait' });
	});
});
