/**
 * Pure post-move board snapshot — TypeScript twin of
 * `blob_domain::simulate_move`. Must stay byte-identical for tile rules so
 * auto-optimism paints the same board the Atomic response will seal.
 *
 * Registered as pure function `blob.simulate_move` on the command runtime.
 */

const HOLE = 0;
const UNVISITED = 1;
const VISITED = 2;
const DEAD_BY_SUICIDE = 3;
const DEAD_BY_HOLE = 4;
const PLAYER = 9;

export type BlobMoveArgs = Readonly<{
	direction: string;
}>;

export type BlobMoveResult = Readonly<{
	map_json: string;
	score: number;
	player_dead: boolean;
	current_level_completed: boolean;
	status: string;
}>;

/**
 * Apply one direction to a known BlobGames row.
 * Returns null when the move is impossible (edge/no map) so optimism fails closed.
 */
export function simulateMove(
	record: Readonly<Record<string, unknown>>,
	args: Readonly<Record<string, unknown>>
): BlobMoveResult | null {
	const direction = args.direction;
	if (typeof direction !== 'string') return null;
	const mapJson = record.map_json;
	if (typeof mapJson !== 'string') return null;
	let map: number[][];
	try {
		map = JSON.parse(mapJson) as number[][];
	} catch {
		return null;
	}
	if (!Array.isArray(map) || map.length === 0 || !Array.isArray(map[0]) || map[0]!.length === 0) {
		return null;
	}
	const scoreRaw = record.score;
	const score =
		typeof scoreRaw === 'number'
			? scoreRaw
			: typeof scoreRaw === 'bigint'
				? Number(scoreRaw)
				: typeof scoreRaw === 'string'
					? Number(scoreRaw)
					: NaN;
	if (!Number.isFinite(score)) return null;

	const pos = playerPos(map);
	if (pos === null) return null;
	const [r, c] = pos;
	const next = step(r, c, direction, map);
	if (next === null) return null;
	const [nr, nc] = next;

	const nextMap = map.map((row) => row.slice());
	let nextScore = score;
	let playerDead = false;
	let levelComplete = false;

	nextMap[r]![c] = VISITED;
	const target = nextMap[nr]![nc]!;
	if (target === HOLE) {
		nextMap[nr]![nc] = DEAD_BY_HOLE;
	} else if (target === VISITED) {
		nextMap[nr]![nc] = DEAD_BY_SUICIDE;
	} else if (target === UNVISITED || target === PLAYER) {
		nextScore += 1;
		nextMap[nr]![nc] = PLAYER;
	} else {
		nextMap[nr]![nc] = DEAD_BY_SUICIDE;
	}

	for (const row of nextMap) {
		if (row.includes(DEAD_BY_HOLE) || row.includes(DEAD_BY_SUICIDE)) {
			playerDead = true;
			levelComplete = false;
			break;
		}
	}
	if (!playerDead) {
		levelComplete = !nextMap.some((row) => row.includes(UNVISITED));
	}

	return Object.freeze({
		map_json: JSON.stringify(nextMap),
		score: nextScore,
		player_dead: playerDead,
		current_level_completed: levelComplete,
		status: playerDead ? 'dead' : levelComplete ? 'level_complete' : 'active'
	});
}

function playerPos(map: number[][]): [number, number] | null {
	for (let r = 0; r < map.length; r += 1) {
		const row = map[r]!;
		for (let c = 0; c < row.length; c += 1) {
			if (row[c] === PLAYER) return [r, c];
		}
	}
	return null;
}

function step(
	r: number,
	c: number,
	direction: string,
	map: number[][]
): [number, number] | null {
	switch (direction) {
		case 'up':
			return r === 0 ? null : [r - 1, c];
		case 'down':
			return r + 1 >= map.length ? null : [r + 1, c];
		case 'left':
			return c === 0 ? null : [r, c - 1];
		case 'right': {
			const row = map[r]!;
			return c + 1 >= row.length ? null : [r, c + 1];
		}
		default:
			return null;
	}
}

/** Pure registry entry for the command runtime. */
export const BLOB_PURE_FUNCTIONS = Object.freeze({
	'blob.simulate_move': simulateMove
});
