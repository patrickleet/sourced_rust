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

function isBinarySorted(ids: string[]) {
	return JSON.stringify(ids) === JSON.stringify([...ids].sort());
}

function validTodoTransitionFrame(frame: TodoOrderFrame, todoId: string) {
	const occurrences =
		frame.open.filter((id) => id === todoId).length +
		frame.done.filter((id) => id === todoId).length;
	return isBinarySorted(frame.open) && isBinarySorted(frame.done) && occurrences === 1;
}

function todoIsIn(frame: TodoOrderFrame, todoId: string, panel: 'open' | 'done') {
	return frame[panel].includes(todoId);
}

function waitForTodoCommand(
	page: import('@playwright/test').Page,
	mutation: 'todos_create' | 'todos_complete' | 'todos_reopen' | 'todos_archive'
) {
	return page.waitForResponse(
		(response) =>
			response.url().includes('/graphql') &&
			(response.request().postData() ?? '').includes(mutation),
		{ timeout: 20_000 }
	);
}

test.describe('todos (alice)', () => {
	test('create, complete, reopen, archive', async ({ page }) => {
		const title = `e2e todo ${Date.now()}`;

		await page.goto('/todos');
		await expect(page.getByRole('heading', { name: /todos/i })).toBeVisible();

		// Create
		await page.locator('#todo-title').fill(title);
		const createResponse = waitForTodoCommand(page, 'todos_create');
		await page.getByRole('button', { name: /^add$/i }).click();
		expect((await createResponse).ok(), 'todos_create must succeed').toBeTruthy();
		const openItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		await expect(openItem).toBeVisible({ timeout: 30_000 });

		// Complete
		const completeResponse = waitForTodoCommand(page, 'todos_complete');
		await openItem.getByRole('button', { name: /^done$/i }).click();
		const doneItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.item', { hasText: title });
		await expect(doneItem).toBeVisible({ timeout: 15_000 });
		expect((await completeResponse).ok(), 'todos_complete must succeed').toBeTruthy();

		// Reopen (prefer the text button; wait until not busy)
		const reopen = doneItem.getByRole('button', { name: 'Reopen', exact: true });
		await expect(reopen).toBeEnabled({ timeout: 10_000 });
		const reopenResponse = waitForTodoCommand(page, 'todos_reopen');
		await reopen.click();
		const openAgain = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^open$/i }) })
			.locator('.item', { hasText: title });
		await expect(openAgain).toBeVisible({ timeout: 15_000 });
		await reopenResponse;

		// Archive
		const archive = openAgain.getByRole('button', { name: 'Archive', exact: true });
		await expect(archive).toBeEnabled({ timeout: 10_000 });
		const archiveResponse = waitForTodoCommand(page, 'todos_archive');
		await archive.click();
		const archiveDetails = page.locator('details.archive');
		await expect(archiveDetails).toBeVisible({ timeout: 15_000 });
		await archiveDetails.locator('summary').click();
		await expect(archiveDetails.locator('.item', { hasText: title })).toBeVisible({
			timeout: 10_000
		});
		// Optimistic visibility is deliberately immediate. Do not tear down the
		// browser runtime until the durable command response has arrived.
		await archiveResponse;

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
				{ timeout: 40_000 }
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
		await expect(page.getByRole('heading', { name: /todos/i })).toBeVisible();

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
		await expect(page.getByRole('heading', { name: /todos/i })).toBeVisible();

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
		// Auto-optimism maps title/owner/todo_id but not domain status constants
		// (`open`/`completed`). Open/Done columns filter on status, so list
		// membership seals with the Eventual payload rather than pre-wire paint.
		// Controls must still stay enabled under the delayed route.
		expect(
			await page.locator('.board button:disabled').count(),
			'routine command concurrency guards must not flash Todo row controls disabled'
		).toBe(0);
		await createResponse;
		await expect(openItem).toBeVisible({ timeout: 5_000 });
		expect(
			await page.locator('.board button:disabled').count(),
			'routine command concurrency guards must not flash Todo row controls disabled'
		).toBe(0);
		expectBinarySorted(await visibleTodoOrders(page));
		const todoId = await openItem.getAttribute('data-todo-id');
		expect(todoId).not.toBeNull();
		await startTodoOrderTrace(page);
		const completeResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('todos_complete')
		);
		// Single click: without status constants in auto-optimism, the row stays
		// `open` until Eventual seals, so a double-click is not client-suppressed.
		await openItem.getByRole('button', { name: /^done$/i }).click();
		const doneItem = page
			.locator('.panel')
			.filter({ has: page.getByRole('heading', { name: /^done$/i }) })
			.locator('.item', { hasText: title });
		expect(
			await page.locator('.board button:disabled').count(),
			'routine command concurrency guards must not flash Todo row controls disabled'
		).toBe(0);
		await completeResponse;
		await expect(doneItem).toBeVisible({ timeout: 5_000 });
		expectBinarySorted(await visibleTodoOrders(page));
		expect(completeRequests, 'complete must reach the server once').toBe(1);
		await expect(doneItem).toBeVisible();
		expectBinarySorted(await visibleTodoOrders(page));
		await page.waitForTimeout(750);
		const completeOrderFrames = await stopTodoOrderTrace(page);
		expect(
			completeOrderFrames.every((order) => validTodoTransitionFrame(order, todoId!)),
			`complete rendered an intermediate non-generated order: ${JSON.stringify(completeOrderFrames)}`
		).toBe(true);
		expect(
			completeOrderFrames.some((order) => todoIsIn(order, todoId!, 'done')),
			`complete never rendered the Todo in Done: ${JSON.stringify(completeOrderFrames)}`
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
		expect(
			reopenOrderFrames.every((order) => validTodoTransitionFrame(order, todoId!)),
			`reopen rendered an intermediate non-generated order: ${JSON.stringify(reopenOrderFrames)}`
		).toBe(true);
		expect(
			reopenOrderFrames.some((order) => todoIsIn(order, todoId!, 'open')),
			`reopen never rendered the Todo in Open: ${JSON.stringify(reopenOrderFrames)}`
		).toBe(true);
		await expect(reopenedItem).toBeVisible();
		expectBinarySorted(await visibleTodoOrders(page));

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
			authoritativeCompleteFrames.every((order) =>
				validTodoTransitionFrame(order, todoId!)
			),
			`authoritative complete rendered an intermediate order: ${JSON.stringify(authoritativeCompleteFrames)}`
		).toBe(true);
		expect(
			authoritativeCompleteFrames.some((order) => todoIsIn(order, todoId!, 'done')),
			`authoritative complete never rendered the Todo in Done: ${JSON.stringify(authoritativeCompleteFrames)}`
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
			authoritativeReopenFrames.every((order) =>
				validTodoTransitionFrame(order, todoId!)
			),
			`authoritative reopen rendered an intermediate order: ${JSON.stringify(authoritativeReopenFrames)}`
		).toBe(true);
		expect(
			authoritativeReopenFrames.some((order) => todoIsIn(order, todoId!, 'open')),
			`authoritative reopen never rendered the Todo in Open: ${JSON.stringify(authoritativeReopenFrames)}`
		).toBe(true);
		await expect(reopenedItem).toBeVisible();
		expectBinarySorted(await visibleTodoOrders(page));
		await page.unrouteAll({ behavior: 'wait' });
	});
});
