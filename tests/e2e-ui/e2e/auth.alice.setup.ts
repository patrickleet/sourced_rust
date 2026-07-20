import { test as setup } from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loginAs } from './helpers/login';

const authDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '.auth');
const aliceFile = path.join(authDir, 'alice.json');

setup('authenticate as alice', async ({ page }) => {
	fs.mkdirSync(authDir, { recursive: true });
	const user = process.env.E2E_HUMAN_ALICE || 'alice';
	await loginAs(page, user, process.env.E2E_HUMAN_PASSWORD || 'Password1!', '/todos');
	await page.waitForURL(/\/todos/, { timeout: 30_000 });
	await page.getByRole('heading', { name: /field notes/i }).waitFor({ timeout: 20_000 });
	await page.context().storageState({ path: aliceFile });
});
