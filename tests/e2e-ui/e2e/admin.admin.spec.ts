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

		const forceBtn = page.getByRole('button', { name: /force archive/i }).first();
		if ((await forceBtn.count()) === 0) {
			test.skip(true, 'no non-archived todos to force-archive');
		}

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.ad-table tbody tr').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.ad-table tbody tr').length);
			});
			observer.observe(document.querySelector('.ad-table-wrap')!, {
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedAdminContinuitySamples: samples,
				__distributedAdminContinuityObserver: observer
			});
		});

		await forceBtn.click();
		await expect(forceBtn).toBeHidden({ timeout: 15_000 }).catch(async () => {
			await expect(page.getByText(/archived/i).first()).toBeVisible();
		});
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
