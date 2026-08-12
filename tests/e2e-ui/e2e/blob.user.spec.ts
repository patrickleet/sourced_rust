import { test, expect } from '@playwright/test';

test.describe('blob game (alice)', () => {
	test('start a game, paint board, move, show in history', async ({ page }) => {
		await page.goto('/blob');
		await expect(page.getByRole('heading', { name: /blob game/i })).toBeVisible();

		// Wait for client hydration so Start/New game handlers are attached.
		await expect(page.locator('[data-blob-hydrated="1"]')).toBeVisible({ timeout: 15_000 });
		await expect(page.getByTestId('blob-start-game')).toBeEnabled({ timeout: 10_000 });

		// Empty state — no fake grid pretending to be a game
		await expect(page.getByText(/no game selected/i)).toBeVisible();
		await expect(page.locator('.blob-board')).toHaveCount(0);

		const start = page.getByTestId('blob-start-game');
		const [resp] = await Promise.all([
			page.waitForResponse(
				(r) =>
					r.url().includes('/graphql') &&
					r.request().method() === 'POST' &&
					(r.request().postData() ?? '').includes('blob_games_start'),
				{ timeout: 20_000 }
			),
			start.click()
		]);
		expect(resp.ok(), `blob_games_start HTTP ${resp.status()}`).toBeTruthy();

		// Real board from command payload
		const board = page.locator('.blob-board');
		await expect(board).toBeVisible({ timeout: 15_000 });
		await expect(page).toHaveURL(/\/blob\/[^/]+$/);
		await expect(board.locator('.cell').first()).toBeVisible();
		await expect(page.locator('.hud-value').first()).toBeVisible();

		// Score starts at 0
		await expect(page.locator('.blob-hud')).toContainText(/score/i);

		// Generated levels start at the top-left. A known boundary is a local
		// no-op: no rejected command and no framework error should escape.
		let wallMoveRequests = 0;
		const countWallMoves = (request: import('@playwright/test').Request) => {
			if (
				request.url().includes('/graphql') &&
				(request.postData() ?? '').includes('blob_games_move')
			) {
				wallMoveRequests += 1;
			}
		};
		page.on('request', countWallMoves);
		await page.keyboard.press('ArrowUp');
		await page.waitForTimeout(250);
		page.off('request', countWallMoves);
		expect(wallMoveRequests, 'a board-edge move should not reach GraphQL').toBe(0);
		await expect(page.locator('.inline-alert')).toHaveCount(0);

		const navigations: string[] = [];
		page.on('framenavigated', (frame) => {
			if (frame === page.mainFrame()) navigations.push(frame.url());
		});
		await page.evaluate(() => {
			const samples = [document.querySelectorAll('.blob-board').length];
			const observer = new MutationObserver(() => {
				samples.push(document.querySelectorAll('.blob-board').length);
			});
			observer.observe(document.querySelector('.blob-page')!, {
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedBlobContinuitySamples: samples,
				__distributedBlobContinuityObserver: observer
			});
		});

		let releaseMove!: () => void;
		const releaseMovePromise = new Promise<void>((resolve) => {
			releaseMove = resolve;
		});
		let moveReachedServer!: () => void;
		const moveReachedServerPromise = new Promise<void>((resolve) => {
			moveReachedServer = resolve;
		});
		await page.route('**/graphql', async (route) => {
			if (!(route.request().postData() ?? '').includes('blob_games_move')) {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			moveReachedServer();
			await releaseMovePromise;
			await route.fulfill({ response });
		});

		// Move right (pad or keyboard). Either score increments or stays at an
		// edge/wall, but the cached board must remain mounted throughout.
		const moveResponsePromise = page.waitForResponse(
			(r) =>
				r.url().includes('/graphql') &&
				r.request().method() === 'POST' &&
				(r.request().postData() ?? '').includes('blob_games_move'),
			{ timeout: 20_000 }
		);
		await page.keyboard.press('ArrowRight');
		await moveReachedServerPromise;
		// Pure reduce (blob.simulate_move) paints the next board from the known
		// cache row + direction before the held GraphQL response returns.
		await expect(board.locator('.tile-player')).toHaveAttribute(
			'aria-label',
			'r0 c1',
			{ timeout: 200 }
		);
		await expect(page.getByTestId('blob-new-game')).toBeEnabled();
		for (const button of await page.locator('.pad-btn').all()) {
			await expect(button).toBeEnabled();
		}
		releaseMove();
		const moveResp = await moveResponsePromise;
		expect(moveResp.ok(), `blob_games_move HTTP ${moveResp.status()}`).toBeTruthy();
		await page.unrouteAll({ behavior: 'wait' });
		await expect(board).toBeVisible();
		await expect(page.locator('.inline-alert, .blob-empty')).toHaveCount(0);

		// History lists this game
		await expect(page.getByRole('heading', { name: /your games/i })).toBeVisible({
			timeout: 15_000
		});
		await expect(page.locator('.history-item').first()).toBeVisible({ timeout: 15_000 });

		// Soft URL may include game id (replaceState) or stay /blob
		await page.waitForTimeout(300);
		expect(page.url()).toMatch(/\/blob/);
		const samples = await page.evaluate(() => {
			const state = globalThis as typeof globalThis & {
				__distributedBlobContinuitySamples: number[];
				__distributedBlobContinuityObserver: MutationObserver;
			};
			state.__distributedBlobContinuityObserver.disconnect();
			return state.__distributedBlobContinuitySamples;
		});
		expect(navigations, 'move commands must not navigate or reload the page').toEqual([]);
		expect(
			Math.min(...samples),
			'stale-while-revalidate must never replace the known board with an empty view'
		).toBeGreaterThan(0);
	});

	test('new game replaces a selected game without a page reload', async ({ page }) => {
		await page.goto('/blob');
		await expect(page.locator('[data-blob-hydrated="1"]')).toBeVisible({
			timeout: 15_000
		});
		await expect(page.getByTestId('blob-start-game')).toBeEnabled({ timeout: 10_000 });

		const startResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('blob_games_start'),
			{ timeout: 20_000 }
		);
		await page.getByTestId('blob-start-game').click();
		expect((await startResponse).ok()).toBeTruthy();
		await expect(page.locator('.blob-board')).toBeVisible({ timeout: 15_000 });
		const firstGameUrl = page.url();

		const continuityToken = `blob-new-game-${Date.now()}`;
		await page.evaluate((token) => {
			Object.assign(globalThis, { __distributedBlobNewGameToken: token });
		}, continuityToken);
		const newGameResponse = page.waitForResponse(
			(response) =>
				(response.request().postData() ?? '').includes('blob_games_start'),
			{ timeout: 20_000 }
		);
		await page.getByTestId('blob-new-game').click();
		expect((await newGameResponse).ok()).toBeTruthy();

		await expect(page).not.toHaveURL(firstGameUrl);
		await expect(page.locator('.blob-board .tile-player')).toHaveAttribute(
			'aria-label',
			'r0 c0',
			{ timeout: 1_000 }
		);
		await expect(page.locator('.blob-empty')).toHaveCount(0);
		expect(
			await page.evaluate(
				() =>
					(globalThis as typeof globalThis & {
						__distributedBlobNewGameToken?: string;
					}).__distributedBlobNewGameToken
			),
			'New game must preserve the current document while changing routes'
		).toBe(continuityToken);
	});

	test('a revalidation started before an atomic move cannot roll it back with later evidence', async ({ page }) => {
		await page.goto('/blob');
		await expect(page.locator('[data-blob-hydrated="1"]')).toBeVisible({ timeout: 15_000 });
		const start = page.getByTestId('blob-start-game');
		await expect(start).toBeEnabled({ timeout: 10_000 });
		const [startResponse] = await Promise.all([
			page.waitForResponse(
				(response) =>
					response.url().includes('/graphql') &&
					(response.request().postData() ?? '').includes('blob_games_start'),
				{ timeout: 20_000 }
			),
			start.click()
		]);
		expect(startResponse.ok()).toBeTruthy();
		await expect(page.locator('.blob-board .tile-player')).toHaveAttribute(
			'aria-label',
			'r0 c0'
		);
		await expect(page).toHaveURL(/\/blob\/[^/]+$/);
		const gameId = decodeURIComponent(new URL(page.url()).pathname.split('/').at(-1)!);

		let releaseHeldQuery!: () => void;
		const heldQueryRelease = new Promise<void>((resolve) => {
			releaseHeldQuery = resolve;
		});
		let heldQueryReady!: () => void;
		const heldQuery = new Promise<void>((resolve) => {
			heldQueryReady = resolve;
		});
		let newerQueryReady!: () => void;
		const newerQuery = new Promise<void>((resolve) => {
			newerQueryReady = resolve;
		});
		let held = false;
		let released = false;
		let raisedHeldRevision = false;
		let newerPlayerLabel: string | null = null;

		await page.route('**/graphql', async (route) => {
			const requestBody = route.request().postData() ?? '';
			if (!requestBody.includes('query BlobGames')) {
				await route.continue();
				return;
			}
			const response = await route.fetch();
			const payload = (await response.json()) as {
				data?: {
					blob_games?: Array<{ game_id: string; map_json: string }>;
				};
				extensions?: {
					distributed?: {
						snapshot?: {
							records?: Array<{
								path?: Array<string | number>;
								revision: string;
							}>;
						};
					};
				};
			};
			const gameIndex =
				payload.data?.blob_games?.findIndex((row) => row.game_id === gameId) ?? -1;
			const game = gameIndex < 0 ? undefined : payload.data?.blob_games?.[gameIndex];
			let playerLabel: string | null = null;
			if (game !== undefined) {
				const rows = JSON.parse(game.map_json) as number[][];
				for (const [rowIndex, row] of rows.entries()) {
					const columnIndex = row.indexOf(9);
					if (columnIndex >= 0) {
						playerLabel = `r${rowIndex} c${columnIndex}`;
						break;
					}
				}
			}
			if (!held && playerLabel === 'r0 c1') {
				held = true;
				const gameRecord = payload.extensions?.distributed?.snapshot?.records?.find(
					(record) =>
						record.path?.[0] === 'blob_games' &&
						Number(record.path[1]) === gameIndex
				);
				if (gameRecord !== undefined) {
					/*
					 * Reproduce the server race: this SQL body was read before
					 * the next command, but response evidence was issued after
					 * it and is therefore numerically newer.
					 */
					gameRecord.revision = '999999999999999999';
					raisedHeldRevision = true;
				}
				heldQueryReady();
				await heldQueryRelease;
			} else if (
				released &&
				newerPlayerLabel !== null &&
				playerLabel === newerPlayerLabel
			) {
				newerQueryReady();
			}
			await route.fulfill({
				response,
				body: JSON.stringify(payload),
				headers: {
					...response.headers(),
					'content-type': 'application/json'
				}
			});
		});

		await page.evaluate(() => {
			const samples: string[] = [];
			const sample = () => {
				const label = document
					.querySelector('.blob-board .tile-player')
					?.getAttribute('aria-label');
				samples.push(label ?? 'missing');
			};
			sample();
			const observer = new MutationObserver(sample);
			observer.observe(document.querySelector('.blob-page')!, {
				attributes: true,
				childList: true,
				subtree: true,
				characterData: true
			});
			Object.assign(globalThis, {
				__distributedBlobRaceSamples: samples,
				__distributedBlobRaceObserver: observer
			});
		});

		const move = async (key: 'ArrowRight' | 'ArrowDown', expectedLabel: string) => {
			const response = page.waitForResponse(
				(candidate) =>
					candidate.url().includes('/graphql') &&
					(candidate.request().postData() ?? '').includes('blob_games_move'),
				{ timeout: 20_000 }
			);
			await page.keyboard.press(key);
			expect((await response).ok()).toBeTruthy();
			await expect(page.locator('.blob-board .tile-player')).toHaveAttribute(
				'aria-label',
				expectedLabel
			);
		};
		const refetch = () =>
			page.evaluate(() => {
				const refetchBlobGames = (
					globalThis as typeof globalThis & {
						__distributedBlobRefetch?: () => Promise<void>;
					}
				).__distributedBlobRefetch;
				if (refetchBlobGames === undefined) {
					throw new Error('Blob refetch test hook is unavailable');
				}
				return refetchBlobGames();
			});

		await move('ArrowRight', 'r0 c1');
		const heldRefetch = refetch();
		await heldQuery;
		expect(
			raisedHeldRevision,
			'the held response must carry deliberately later record evidence'
		).toBe(true);
		const rightClass =
			(await page
				.locator('.blob-board .cell[aria-label="r0 c2"]')
				.getAttribute('class')) ?? '';
		const nextMove = rightClass.includes('tile-hole')
			? ({ key: 'ArrowDown', label: 'r1 c1' } as const)
			: ({ key: 'ArrowRight', label: 'r0 c2' } as const);
		await expect(
			page.locator(`.blob-board .cell[aria-label="${nextMove.label}"]`)
		).not.toHaveClass(/tile-hole/);
		newerPlayerLabel = nextMove.label;
		await move(nextMove.key, nextMove.label);
		const releaseSampleIndex = await page.evaluate(
			() =>
				(
					globalThis as typeof globalThis & {
						__distributedBlobRaceSamples: string[];
					}
				).__distributedBlobRaceSamples.length
		);
		released = true;
		releaseHeldQuery();
		await heldRefetch;
		await Promise.all([newerQuery, refetch()]);
		await expect(page.locator('.blob-board .tile-player')).toHaveAttribute(
			'aria-label',
			nextMove.label
		);

		const postReleaseSamples = await page.evaluate((startIndex) => {
			const state = globalThis as typeof globalThis & {
				__distributedBlobRaceSamples: string[];
				__distributedBlobRaceObserver: MutationObserver;
			};
			state.__distributedBlobRaceObserver.disconnect();
			return state.__distributedBlobRaceSamples.slice(startIndex);
		}, releaseSampleIndex);
		expect(
			postReleaseSamples,
			'an older query response must never hide or move the projected player'
		).not.toContain('missing');
		expect(postReleaseSamples).not.toContain('r0 c1');

		await page.reload();
		await expect(page.locator('.blob-board .tile-player')).toHaveAttribute(
			'aria-label',
			nextMove.label
		);
		await page.unrouteAll({ behavior: 'wait' });
	});
});
