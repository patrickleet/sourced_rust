import { test, expect } from '@playwright/test';

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
});
