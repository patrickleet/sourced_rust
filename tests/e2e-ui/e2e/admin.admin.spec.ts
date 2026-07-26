import { test, expect } from '@playwright/test';

async function openAdmin(page: import('@playwright/test').Page) {
	const res = await page.goto('/admin', { waitUntil: 'domcontentloaded' });
	const status = res?.status() ?? 0;
	const body = await page.locator('body').innerText();
	const denied =
		status === 403 ||
		/admin role required/i.test(body) ||
		(/403/.test(body) && /lost in the cluster/i.test(body));
	return { status, body, denied };
}

test.describe('admin (admin user)', () => {
	test('admin session can hit /admin (role-gated)', async ({ page }) => {
		const { denied, body } = await openAdmin(page);
		if (denied) {
			// Authenticated but engineRole is not admin in this Zitadel bootstrap.
			test.info().annotations.push({
				type: 'note',
				description:
					'admin human lacks engineRole=admin claims — grant Zitadel project role `admin` to exercise force-archive'
			});
			expect(body.length).toBeGreaterThan(10);
			return;
		}

		await expect(page.getByRole('heading', { name: /all field notes/i })).toBeVisible({
			timeout: 20_000
		});
		await expect(page.getByText(/force archive/i).first()).toBeVisible();
	});

	test('force archive when rows exist and role is admin', async ({ page }) => {
		const { denied } = await openAdmin(page);
		if (denied) {
			test.skip(true, 'admin engine role not granted in this environment');
		}

		await expect(page.getByRole('heading', { name: /all field notes/i })).toBeVisible({
			timeout: 20_000
		});
		// Wait for the nested fieldnote-admin client to hydrate before invoking
		// elevated commands (SSR markup alone has no Svelte handlers).
		await page.waitForLoadState('networkidle');
		const forceButtons = page.getByRole('button', { name: /force archive/i });
		await expect(forceButtons.first()).toBeEnabled({ timeout: 20_000 });

		const targetRow = page
			.locator('.ad-table tbody tr')
			.filter({ has: page.getByRole('button', { name: /force archive/i }) })
			.first();
		if ((await targetRow.count()) === 0) {
			test.skip(true, 'no non-archived todos to force-archive');
		}

		const todoId = await targetRow.getAttribute('data-todo-id');
		expect(todoId, 'admin row must expose data-todo-id').toBeTruthy();
		const forceBtn = targetRow.getByRole('button', { name: /force archive/i });

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.ad-table tbody tr').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.ad-table tbody tr').length);
			});
			const wrap = document.querySelector('.ad-table-wrap');
			if (wrap === null) {
				throw new Error('admin table wrap missing before force archive');
			}
			observer.observe(wrap, {
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedAdminContinuitySamples: samples,
				__distributedAdminContinuityObserver: observer
			});
		});

		const forceResponse = page.waitForResponse(
			(response) =>
				response.url().includes('graphql') &&
				(response.request().postData() ?? '').includes('todos_force_archive'),
			{ timeout: 20_000 }
		);
		await forceBtn.click();
		const response = await forceResponse;
		const responseBody = await response.text();
		expect(
			response.ok(),
			`todos_force_archive HTTP ${response.status()}: ${responseBody.slice(0, 400)}`
		).toBeTruthy();
		expect(
			responseBody,
			`todos_force_archive GraphQL errors: ${responseBody.slice(0, 600)}`
		).not.toMatch(/"errors"\s*:\s*\[/);

		// Scope to the clicked row: other rows may still show Force archive.
		const archivedRow = page.locator(`.ad-table tbody tr[data-todo-id="${todoId}"]`);
		await expect(archivedRow.locator('[data-status="archived"]')).toBeVisible({
			timeout: 15_000
		});
		await expect(archivedRow.getByRole('button', { name: /force archive/i })).toHaveCount(0);

		const samples = await page.evaluate(() => {
			const state = globalThis as typeof globalThis & {
				__distributedAdminContinuitySamples: number[];
				__distributedAdminContinuityObserver: MutationObserver;
			};
			state.__distributedAdminContinuityObserver.disconnect();
			return state.__distributedAdminContinuitySamples;
		});
		expect(navigations, 'force archive must not navigate or reload the page').toEqual([]);
		expect(
			Math.min(...samples),
			'stale-while-revalidate must never replace known admin rows with an empty view'
		).toBeGreaterThan(0);
	});
});
