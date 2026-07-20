import { test as setup, expect } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loginAs } from './helpers/login';

const authDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '.auth');
const adminFile = path.join(authDir, 'admin.json');

setup('authenticate as admin', async ({ page }) => {
	fs.mkdirSync(authDir, { recursive: true });
	// Land on todos first so login always has a real destination; admin role is checked on /admin.
	await loginAs(page, 'admin', process.env.E2E_HUMAN_PASSWORD || 'Password1!', '/todos');
	await page.waitForURL(/\/todos/, { timeout: 30_000 });
	await expect(page.getByRole('heading', { name: /field notes/i })).toBeVisible({
		timeout: 20_000
	});
	await page.context().storageState({ path: adminFile });
});
