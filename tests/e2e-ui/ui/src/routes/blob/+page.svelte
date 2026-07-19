<script lang="ts">
	/**
	 * Blob Game — optimistic client + command returns RM row.
	 *
	 * 1) Paint immediately (client rules).
	 * 2) Queue command (server writes RM, returns row).
	 * 3) Reconcile board ONLY when no unconfirmed optimistics remain —
	 *    never apply an older command response over a newer local board
	 *    (that was the undo/redo flicker).
	 */
	import { onDestroy, onMount } from 'svelte';
	import { useGraphql, fx } from '$lib/gql';
	import { blobGames } from './blob.resource';
	import type { BlobGameRow } from './blob.resource';

	const TILE = {
		player: 9,
		hole: 0,
		unvisited: 1,
		visited: 2,
		deadBySuicide: 3,
		deadByHole: 4
	} as const;

	let { data } = $props();

	let actionError = $state<string | null>(null);
	let starting = $state(false);
	let activeId = $state<string | null>(null);

	// Local play state — optimistic first, then command payload
	let board = $state<number[][]>([]);
	let score = $state(0);
	let playerDead = $state(false);
	let levelComplete = $state(false);
	let currentLevel = $state(0);
	let status = $state('active');
	let playGameId = $state<string | null>(null);

	/** Serialize network so aggregate versions stay ordered. */
	let moveQueue: string[] = [];
	let draining = false;
	/** Optimistic moves not yet confirmed by a successful command response. */
	let pendingConfirms = 0;
	/** Last server-confirmed row (for rollback on error). */
	let lastConfirmed: BlobGameRow | null = null;

	const gql = useGraphql(() => data, {
		runEffects: (effects) => {
			for (const e of effects) {
				if (e.kind === 'alert') actionError = e.message;
			}
		}
	});

	const list = gql.store({
		document: blobGames.query,
		list: { at: 'blob_games', by: 'game_id' },
		initialData: { blob_games: data.games ?? [] },
		select: (d: { blob_games?: BlobGameRow[] }) => d?.blob_games ?? []
	});

	$effect(() => {
		list.seed({ blob_games: data.games ?? [] });
	});

	onDestroy(() => list.destroy());

	const games = $derived(($list?.data as BlobGameRow[] | undefined) ?? []);

	const serverActive = $derived.by((): BlobGameRow | null => {
		if (!games.length) return null;
		if (activeId) return games.find((g) => g.game_id === activeId) ?? games[0] ?? null;
		return games[0] ?? null;
	});

	function seedList(row: BlobGameRow) {
		const rest = games.filter((g) => g.game_id !== row.game_id);
		list.seed({ blob_games: [row, ...rest] });
	}

	/**
	 * Apply server/command row to the board.
	 * @param force — overwrite optimistic board (start, error rollback, select game).
	 *   When false, only paints if we're fully caught up (pendingConfirms === 0).
	 */
	function applyRow(row: BlobGameRow, force = true) {
		playGameId = row.game_id;
		activeId = row.game_id;
		seedList(row);
		lastConfirmed = row;

		if (!force && pendingConfirms > 0) {
			// Still optimistically ahead — keep local board, don't rewind
			return;
		}

		score = Number(row.score) || 0;
		playerDead = !!row.player_dead;
		levelComplete = !!row.current_level_completed;
		currentLevel = Number(row.current_level) || 0;
		status = row.status || 'active';
		try {
			const m = JSON.parse(row.map_json || '[]') as number[][];
			board = Array.isArray(m) ? m.map((r) => [...r]) : [];
		} catch {
			board = [];
		}
	}

	$effect(() => {
		const row = serverActive;
		// Only auto-hydrate empty play session — never clobber active optimistics
		if (row && !playGameId && pendingConfirms === 0) applyRow(row, true);
	});

	const cols = $derived(board[0]?.length ?? 0);
	const hasBoard = $derived(board.length > 0 && cols > 0);

	function newGameId() {
		const rand =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID().replace(/-/g, '').slice(0, 12)
				: `${Date.now().toString(16)}${Math.random().toString(16).slice(2, 8)}`;
		return `blob-${rand}`;
	}

	function playerPos(map: number[][]): { r: number; c: number } | null {
		for (let r = 0; r < map.length; r++) {
			const c = map[r].indexOf(TILE.player);
			if (c >= 0) return { r, c };
		}
		return null;
	}

	function evaluateWinLose(map: number[][]): { dead: boolean; complete: boolean } {
		for (const row of map) {
			if (row.includes(TILE.deadByHole) || row.includes(TILE.deadBySuicide)) {
				return { dead: true, complete: false };
			}
		}
		const anyUnvisited = map.some((row) => row.includes(TILE.unvisited));
		return { dead: false, complete: !anyUnvisited };
	}

	/** Client-side rules (mirror aggregate) for optimistic paint only. */
	function optimisticMove(
		mapIn: number[][],
		direction: string,
		scoreIn: number
	): { map: number[][]; score: number; dead: boolean; complete: boolean } | null {
		const map = mapIn.map((row) => [...row]);
		const pos = playerPos(map);
		if (!pos) return null;
		let nr = pos.r;
		let nc = pos.c;
		switch (direction) {
			case 'up':
				if (pos.r === 0) return null;
				nr = pos.r - 1;
				break;
			case 'down':
				if (pos.r >= map.length - 1) return null;
				nr = pos.r + 1;
				break;
			case 'left':
				if (pos.c === 0) return null;
				nc = pos.c - 1;
				break;
			case 'right':
				if (pos.c >= map[pos.r].length - 1) return null;
				nc = pos.c + 1;
				break;
			default:
				return null;
		}

		map[pos.r][pos.c] = TILE.visited;
		const dest = map[nr][nc];
		let nextScore = scoreIn;
		if (dest === TILE.hole) {
			map[nr][nc] = TILE.deadByHole;
		} else if (dest === TILE.visited) {
			map[nr][nc] = TILE.deadBySuicide;
		} else if (dest === TILE.unvisited || dest === TILE.player) {
			nextScore += 1;
			map[nr][nc] = TILE.player;
		} else {
			map[nr][nc] = TILE.deadBySuicide;
		}

		const { dead, complete } = evaluateWinLose(map);
		return { map, score: nextScore, dead, complete };
	}

	function tileClass(t: number): string {
		switch (t) {
			case TILE.player:
				return 'tile-player';
			case TILE.hole:
				return 'tile-hole';
			case TILE.unvisited:
				return 'tile-unvisited';
			case TILE.visited:
				return 'tile-visited';
			case TILE.deadBySuicide:
				return 'tile-dead-suicide';
			case TILE.deadByHole:
				return 'tile-dead-hole';
			default:
				return 'tile-unknown';
		}
	}

	function tileLabel(t: number): string {
		if (t === TILE.player) return '●';
		if (t === TILE.deadByHole || t === TILE.deadBySuicide) return '✕';
		return '';
	}

	async function startGame() {
		if (starting) return;
		starting = true;
		actionError = null;
		const game_id = newGameId();
		const result = await gql.commands.blobGamesStart(
			{ game_id },
			{ onError: ({ errors }) => [fx.alert(errors[0]?.message ?? 'Start failed')] }
		);
		starting = false;
		if (result.errors?.length || !result.data) {
			if (!actionError) actionError = result.errors?.[0]?.message ?? 'Start failed';
			return;
		}
		// Command wrote RM; payload is the row
		pendingConfirms = 0;
		applyRow(result.data as BlobGameRow, true);
	}

	async function drainMoveQueue(game_id: string) {
		if (draining) return;
		draining = true;
		while (moveQueue.length) {
			const direction = moveQueue.shift()!;
			try {
				const result = await gql.commands.blobGamesMove({ game_id, direction });
				if (result.errors?.length || !result.data) {
					actionError = result.errors?.[0]?.message ?? 'Move rejected';
					moveQueue = [];
					pendingConfirms = 0;
					// Roll back to last confirmed RM (or list)
					if (lastConfirmed?.game_id === game_id) applyRow(lastConfirmed, true);
					else {
						const row = games.find((g) => g.game_id === game_id);
						if (row) applyRow(row, true);
						else list.scheduleCatchUp(80);
					}
					break;
				}
				// One server confirm for one optimistic move
				pendingConfirms = Math.max(0, pendingConfirms - 1);
				const fact = result.data as BlobGameRow;
				// Only paint when fully caught up — intermediate responses must not rewind
				applyRow(fact, pendingConfirms === 0);
			} catch {
				actionError = 'Move failed';
				moveQueue = [];
				pendingConfirms = 0;
				if (lastConfirmed?.game_id === game_id) applyRow(lastConfirmed, true);
				break;
			}
		}
		draining = false;
	}

	/**
	 * Optimistic first (instant), then queued command that returns RM row.
	 * Responses never overwrite a board that is still optimistically ahead.
	 */
	function move(direction: string) {
		if (!playGameId || playerDead || levelComplete || !hasBoard) return;

		const next = optimisticMove(board, direction, score);
		if (!next) return;

		// 1) Instant paint
		board = next.map;
		score = next.score;
		playerDead = next.dead;
		levelComplete = next.complete;
		status = next.dead ? 'dead' : next.complete ? 'level_complete' : 'active';
		actionError = null;
		pendingConfirms += 1;

		// 2) Queue server confirm (mutation writes RM + returns row)
		const game_id = playGameId;
		moveQueue.push(direction);
		void drainMoveQueue(game_id);
	}

	async function nextLevel() {
		if (!playGameId || !levelComplete || playerDead || starting) return;
		starting = true;
		actionError = null;
		const result = await gql.commands.blobGamesStartLevel(
			{ game_id: playGameId },
			{ onError: ({ errors }) => [fx.alert(errors[0]?.message ?? 'Next level failed')] }
		);
		starting = false;
		if (result.errors?.length || !result.data) {
			if (!actionError) actionError = result.errors?.[0]?.message ?? 'Next level failed';
			return;
		}
		applyRow(result.data as BlobGameRow);
	}

	function selectGame(id: string) {
		const row = games.find((g) => g.game_id === id);
		if (row) applyRow(row);
	}

	function onKey(e: KeyboardEvent) {
		const map: Record<string, string> = {
			ArrowUp: 'up',
			ArrowDown: 'down',
			ArrowLeft: 'left',
			ArrowRight: 'right',
			w: 'up',
			s: 'down',
			a: 'left',
			d: 'right',
			W: 'up',
			S: 'down',
			A: 'left',
			D: 'right'
		};
		const dir = map[e.key];
		if (dir) {
			e.preventDefault();
			move(dir);
		}
	}

	onMount(() => {
		window.addEventListener('keydown', onKey);
		return () => window.removeEventListener('keydown', onKey);
	});
</script>

<svelte:head>
	<title>Blob Game · e2e-ui</title>
</svelte:head>

<section class="fn-page blob-page">
	<header class="fn-header">
		<div class="fn-kicker">
			<span class="fn-dot" aria-hidden="true"></span>
			Optimistic UI · command returns RM row
		</div>
		<div class="blob-title-row">
			<div>
				<h1 class="fn-title">Blob Game</h1>
				<p class="fn-lede">
					Board updates instantly (optimistic). The command upserts <code>blob_games</code> and
					returns that row so we reconcile without waiting on projectors.
				</p>
			</div>
			<button class="fn-btn fn-btn-primary" type="button" onclick={startGame} disabled={starting}>
				New game
			</button>
		</div>
	</header>

	{#if data.gqlError}
		<div class="fn-alert" role="alert">
			<span class="fn-alert-label">SSR GraphQL</span>
			{data.gqlError}
		</div>
	{/if}
	{#if actionError}
		<div class="fn-alert" role="alert">
			<span class="fn-alert-label">Command</span>
			{actionError}
		</div>
	{/if}

	{#if !hasBoard}
		<div class="blob-empty">
			<div class="blob-empty-board" aria-hidden="true">
				{#each Array(5) as _, r}
					{#each Array(5) as _, c}
						<span class="cell tile-unvisited" class:tile-player={r === 0 && c === 0}></span>
					{/each}
				{/each}
			</div>
			<p class="blob-empty-copy">
				Start a game, then move with arrows / WASD — paint is optimistic, server confirms via
				command payload.
			</p>
			<button class="fn-btn fn-btn-primary" type="button" onclick={startGame} disabled={starting}>
				Start demo level
			</button>
		</div>
	{:else}
		<div class="blob-stage">
			<div class="blob-hud">
				<div class="hud-stat">
					<span class="hud-label">Score</span>
					<strong class="hud-value">{score}</strong>
				</div>
				<div class="hud-stat">
					<span class="hud-label">Level</span>
					<strong class="hud-value">{currentLevel}</strong>
				</div>
				<div class="hud-stat">
					<span class="hud-label">Status</span>
					<strong class="hud-value hud-status-{status}">{status}</strong>
				</div>
				{#if playerDead}
					<span class="hud-banner dead">You died — start a new game</span>
				{:else if levelComplete}
					<span class="hud-banner win">Level complete</span>
					<button class="fn-btn fn-btn-primary" type="button" onclick={nextLevel} disabled={starting}>
						Next level
					</button>
				{/if}
			</div>

			<div class="blob-board" style="--cols: {cols}" role="grid" aria-label="Blob game board">
				{#each board as row, r}
					{#each row as cell, c}
						<div class="cell {tileClass(cell)}" role="gridcell" aria-label="r{r} c{c}">
							{tileLabel(cell)}
						</div>
					{/each}
				{/each}
			</div>

			<div class="blob-legend" aria-hidden="true">
				<span><i class="swatch tile-player"></i> You</span>
				<span><i class="swatch tile-unvisited"></i> Unvisited</span>
				<span><i class="swatch tile-visited"></i> Visited</span>
				<span><i class="swatch tile-hole"></i> Hole</span>
				<span><i class="swatch tile-dead-hole"></i> Death</span>
			</div>

			<div class="blob-pad" aria-label="Move controls">
				<button type="button" class="pad-btn" onclick={() => move('up')} disabled={playerDead || levelComplete}
					>↑</button
				>
				<div class="pad-row">
					<button
						type="button"
						class="pad-btn"
						onclick={() => move('left')}
						disabled={playerDead || levelComplete}>←</button
					>
					<button
						type="button"
						class="pad-btn"
						onclick={() => move('down')}
						disabled={playerDead || levelComplete}>↓</button
					>
					<button
						type="button"
						class="pad-btn"
						onclick={() => move('right')}
						disabled={playerDead || levelComplete}>→</button
					>
				</div>
			</div>

		</div>
	{/if}

	{#if games.length > 1}
		<section class="blob-history">
			<h2>Your games</h2>
			<ul>
				{#each games as g (g.game_id)}
					<li>
						<button
							type="button"
							class="history-item"
							class:active={g.game_id === playGameId}
							onclick={() => selectGame(g.game_id)}
						>
							<span class="history-id">{g.game_id.slice(0, 14)}…</span>
							<span>score {g.score}</span>
							<span class="hud-status-{g.status}">{g.status}</span>
						</button>
					</li>
				{/each}
			</ul>
		</section>
	{/if}
</section>

<style>
	.blob-page {
		position: relative;
		max-width: 48rem;
		margin: 0 auto;
		padding: 6.5rem 1.25rem 4rem;
		color: var(--wf-ink, #1c1c1a);
	}
	.blob-title-row {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		justify-content: space-between;
		gap: 1rem;
	}
	.fn-title {
		margin: 0 0 0.4rem;
		font-family: var(--wf-serif, Georgia, serif);
		font-size: clamp(1.75rem, 4vw, 2.15rem);
		font-weight: 500;
		letter-spacing: -0.02em;
	}
	.fn-lede {
		margin: 0;
		max-width: 34rem;
		font-size: 0.95rem;
		line-height: 1.5;
		color: var(--wf-ink-soft, #5c5c56);
	}
	.fn-kicker {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--wf-ink-soft, #5c5c56);
		margin-bottom: 0.65rem;
	}
	.fn-dot {
		width: 0.45rem;
		height: 0.45rem;
		border-radius: 50%;
		background: var(--wf-accent, #3d5a80);
	}
	.fn-btn {
		appearance: none;
		border: none;
		border-radius: 8px;
		padding: 0.65rem 1.1rem;
		font: inherit;
		font-weight: 600;
		font-size: 0.92rem;
		cursor: pointer;
		flex-shrink: 0;
	}
	.fn-btn-primary {
		background: var(--wf-accent, #3d5a80);
		color: #fff;
	}
	.fn-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.fn-alert {
		margin: 0 0 1rem;
		padding: 0.75rem 1rem;
		border-radius: 8px;
		background: rgba(179, 58, 58, 0.08);
		border: 1px solid rgba(179, 58, 58, 0.28);
		color: var(--wf-danger, #b33a3a);
		font-size: 0.9rem;
	}
	.fn-alert-label {
		display: block;
		font-size: 0.7rem;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		margin-bottom: 0.25rem;
	}
	.blob-empty {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 1rem;
		padding: 2rem 1rem 2.5rem;
		border: 1px dashed var(--wf-line-strong, #cdcabe);
		border-radius: 12px;
		background: var(--wf-bg-elevated, #fff);
	}
	.blob-empty-board {
		display: grid;
		grid-template-columns: repeat(5, 1.75rem);
		gap: 3px;
		padding: 0.5rem;
		background: #1c1c1a;
		border-radius: 8px;
	}
	.blob-empty-copy {
		margin: 0;
		max-width: 28rem;
		font-size: 0.92rem;
		color: var(--wf-ink-soft, #5c5c56);
		line-height: 1.45;
	}
	.blob-stage {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 1.1rem;
	}
	.blob-hud {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: center;
		gap: 0.75rem 1.25rem;
		width: 100%;
		padding: 0.85rem 1rem;
		border-radius: 10px;
		background: var(--wf-bg-elevated, #fff);
		border: 1px solid var(--wf-line, #e2e0d9);
	}
	.hud-stat {
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
		min-width: 4.5rem;
	}
	.hud-label {
		font-size: 0.68rem;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--wf-ink-muted, #8a8a82);
	}
	.hud-value {
		font-size: 1.15rem;
		font-variant-numeric: tabular-nums;
	}
	.hud-status-active {
		color: var(--wf-accent, #3d5a80);
	}
	.hud-status-dead {
		color: var(--wf-danger, #b33a3a);
	}
	.hud-status-level_complete {
		color: var(--wf-success, #2f6f4e);
	}
	.hud-banner {
		font-weight: 700;
		font-size: 0.9rem;
		padding: 0.35rem 0.65rem;
		border-radius: 6px;
	}
	.hud-banner.dead {
		background: rgba(179, 58, 58, 0.12);
		color: var(--wf-danger, #b33a3a);
	}
	.hud-banner.win {
		background: rgba(47, 111, 78, 0.12);
		color: var(--wf-success, #2f6f4e);
	}
	.blob-board {
		display: grid;
		grid-template-columns: repeat(var(--cols), minmax(2.5rem, 3rem));
		gap: 4px;
		padding: 0.65rem;
		background: #141412;
		border-radius: 12px;
		box-shadow: 0 16px 40px rgba(0, 0, 0, 0.18);
		width: max-content;
		max-width: 100%;
		contain: layout style;
	}
	.cell {
		aspect-ratio: 1;
		min-width: 2.5rem;
		min-height: 2.5rem;
		display: flex;
		align-items: center;
		justify-content: center;
		border-radius: 5px;
		font-size: 1rem;
		font-weight: 700;
	}
	.tile-player {
		background: linear-gradient(145deg, #7eb6ff, #4a8fe0);
		color: #0a1628;
		box-shadow: 0 0 0 2px rgba(126, 182, 255, 0.45);
	}
	.tile-hole {
		background: radial-gradient(circle at 40% 35%, #2a2a28 0%, #0a0a09 70%);
		box-shadow: inset 0 2px 6px rgba(0, 0, 0, 0.6);
	}
	.tile-unvisited {
		background: #3f6b45;
	}
	.tile-visited {
		background: #243528;
	}
	.tile-dead-suicide,
	.tile-dead-hole {
		background: #a33a3a;
		color: #fff;
	}
	.tile-unknown {
		background: #444;
	}
	.blob-legend {
		display: flex;
		flex-wrap: wrap;
		justify-content: center;
		gap: 0.75rem 1.1rem;
		font-size: 0.78rem;
		color: var(--wf-ink-soft, #5c5c56);
	}
	.blob-legend span {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
	}
	.swatch {
		display: inline-block;
		width: 0.85rem;
		height: 0.85rem;
		border-radius: 3px;
	}
	.blob-pad {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.4rem;
	}
	.pad-row {
		display: flex;
		gap: 0.4rem;
	}
	.pad-btn {
		width: 3rem;
		height: 3rem;
		border-radius: 8px;
		border: 1px solid var(--wf-line-strong, #cdcabe);
		background: #fff;
		font-size: 1.2rem;
		cursor: pointer;
		touch-action: manipulation;
	}
	.pad-btn:active:not(:disabled) {
		transform: scale(0.96);
	}
	.pad-btn:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	.blob-history {
		margin-top: 2.5rem;
		padding-top: 1.5rem;
		border-top: 1px solid var(--wf-line, #e2e0d9);
	}
	.blob-history h2 {
		margin: 0 0 0.75rem;
		font-size: 0.95rem;
	}
	.blob-history ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}
	.history-item {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem 1rem;
		width: 100%;
		text-align: left;
		appearance: none;
		border: 1px solid var(--wf-line, #e2e0d9);
		background: #fff;
		border-radius: 8px;
		padding: 0.55rem 0.75rem;
		font: inherit;
		font-size: 0.85rem;
		cursor: pointer;
	}
	.history-item.active {
		border-color: var(--wf-accent, #3d5a80);
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		font-weight: 600;
	}
	.history-id {
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.8rem;
	}
	@media (max-width: 480px) {
		.blob-board {
			grid-template-columns: repeat(var(--cols), minmax(2rem, 1fr));
			width: 100%;
		}
		.cell {
			min-width: 0;
			min-height: 0;
		}
	}
</style>
