import { test, expect } from '@playwright/test';

test.describe('unauthenticated access', () => {
	test('protected routes redirect toward login / OIDC', async ({ page }) => {
		for (const path of ['/todos', '/blob', '/admin', '/session']) {
			await page.goto(path, { waitUntil: 'domcontentloaded' });
			// hooks → /login?callbackUrl=… → may immediately start OIDC
			await page.waitForURL(
				(url) =>
					url.pathname.includes('/login') ||
					url.pathname.includes('/signin') ||
					url.pathname.includes('/auth') ||
					url.hostname.includes('18080') ||
					url.pathname.includes('/oauth'),
				{ timeout: 45_000 }
			);
		}
	});

	test('public chat is reachable without a session', async ({ page }) => {
		await page.goto('/chat');
		await expect(page).toHaveURL(/\/chat(?:[/?#]|$)/);
		await expect(page.getByRole('heading', { name: 'Lobby' })).toBeVisible({
			timeout: 20_000
		});
	});

	test('public layout island survives child navigation and closes on exit', async ({ page }) => {
		const chatSubscriptions = new Set<string>();
		const completed = new Set<string>();
		let browserChatFetches = 0;
		page.on('request', (request) => {
			if (
				request.url().endsWith('/graphql') &&
				(request.postData() ?? '').includes('ChatMessages')
			) {
				browserChatFetches += 1;
			}
		});
		page.on('websocket', (socket) => {
			socket.on('framesent', ({ payload }) => {
				if (typeof payload !== 'string') return;
				let frame: { type?: string; id?: string; payload?: { query?: string } };
				try {
					frame = JSON.parse(payload);
				} catch {
					return;
				}
				if (
					frame.type === 'subscribe' &&
					typeof frame.id === 'string' &&
					frame.payload?.query?.includes('ChatMessages')
				) {
					chatSubscriptions.add(frame.id);
				}
				if (frame.type === 'complete' && typeof frame.id === 'string') {
					completed.add(frame.id);
				}
			});
		});

		await page.goto('/chat');
		await expect(page.getByRole('heading', { name: /^lobby$/i })).toBeVisible();
		await expect.poll(() => chatSubscriptions.size).toBe(1);
		const subscriptionId = [...chatSubscriptions][0];
		expect(browserChatFetches, 'complete SSR hydration must avoid a mount fetch').toBe(0);

		await page.getByTestId('chat-child-link').click();
		await expect(page).toHaveURL(/\/chat\/about$/);
		expect(chatSubscriptions.size).toBe(1);
		expect(completed.has(subscriptionId)).toBe(false);

		await page.getByRole('link', { name: /back to the lobby/i }).click();
		await expect(page).toHaveURL(/\/chat$/);
		expect(chatSubscriptions.size).toBe(1);
		expect(browserChatFetches).toBe(0);

		await page.getByLabel('main navigation').getByRole('link', { name: 'Home' }).click();
		await expect(page).toHaveURL(/\/$/);
		await expect.poll(() => completed.has(subscriptionId)).toBe(true);
	});

	test('home soft-navigation installs the anonymous chat client', async ({ page }) => {
		await page.goto('/');
		await page.waitForLoadState('networkidle');
		const continuityToken = `anonymous-chat-${Date.now()}`;
		await page.evaluate((token) => {
			Object.assign(globalThis, { __anonymousChatContinuityToken: token });
		}, continuityToken);

		await page
			.getByLabel('main navigation')
			.getByRole('link', { name: 'Chat', exact: true })
			.click();

		await expect(page).toHaveURL(/\/chat(?:[/?#]|$)/);
		await expect(page.getByRole('heading', { name: 'Lobby' })).toBeVisible({
			timeout: 20_000
		});
		expect(
			await page.evaluate(
				() =>
					(globalThis as typeof globalThis & {
						__anonymousChatContinuityToken?: string;
					}).__anonymousChatContinuityToken
			),
			'Chat navigation must preserve the current document'
		).toBe(continuityToken);
	});

	test('home page is reachable without a session', async ({ page }) => {
		await page.goto('/');
		await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible({
			timeout: 20_000
		});
	});
});
