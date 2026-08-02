/**
 * Pure move rules — byte-identical twin of `blob_domain::simulate_move`.
 *
 * Used only to fill command **input** fields for the same `.applies` /
 * optimistic-layer path as chat/todos (not a page-local board overlay).
 * The server still recomputes authoritatively from `game_id` + `direction`.
 */

export const TILE = {
	hole: 0,
	unvisited: 1,
	visited: 2,
	deadBySuicide: 3,
	deadByHole: 4,
	player: 9
} as const;

export type Direction = 'up' | 'down' | 'left' | 'right';

export type MovePreview = {
	readonly map: number[][];
	readonly score: number;
	readonly player_dead: boolean;
	readonly level_complete: boolean;
	readonly status: string;
	readonly map_json: string;
};

export class SimulateMoveError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'SimulateMoveError';
	}
}

function statusOf(playerDead: boolean, levelComplete: boolean): string {
	if (playerDead) return 'dead';
	if (levelComplete) return 'level_complete';
	return 'active';
}

function playerPos(map: number[][]): { r: number; c: number } {
	for (let r = 0; r < map.length; r += 1) {
		const row = map[r]!;
		for (let c = 0; c < row.length; c += 1) {
			if (row[c] === TILE.player) return { r, c };
		}
	}
	throw new SimulateMoveError('no active level');
}

/**
 * Apply one direction to a map + score. Mirrors `blob_domain::simulate_move`.
 */
export function simulateMove(
	map: number[][],
	score: number,
	direction: Direction
): MovePreview {
	if (map.length === 0 || (map[0]?.length ?? 0) === 0) {
		throw new SimulateMoveError('no active level');
	}
	const { r, c } = playerPos(map);
	let nr: number;
	let nc: number;
	switch (direction) {
		case 'up':
			if (r === 0) throw new SimulateMoveError('row already 0');
			nr = r - 1;
			nc = c;
			break;
		case 'down':
			if (r + 1 >= map.length) throw new SimulateMoveError('already at bottom edge');
			nr = r + 1;
			nc = c;
			break;
		case 'left':
			if (c === 0) throw new SimulateMoveError('column already 0');
			nr = r;
			nc = c - 1;
			break;
		case 'right':
			if (c + 1 >= (map[r]?.length ?? 0)) {
				throw new SimulateMoveError('already at right edge');
			}
			nr = r;
			nc = c + 1;
			break;
		default: {
			const _exhaustive: never = direction;
			throw new SimulateMoveError(`invalid direction: ${_exhaustive}`);
		}
	}

	const nextMap = map.map((row) => [...row]);
	let nextScore = score;
	let playerDead = false;
	let levelComplete = false;

	nextMap[r]![c] = TILE.visited;
	const landing = nextMap[nr]![nc]!;
	if (landing === TILE.hole) {
		nextMap[nr]![nc] = TILE.deadByHole;
	} else if (landing === TILE.visited) {
		nextMap[nr]![nc] = TILE.deadBySuicide;
	} else if (landing === TILE.unvisited || landing === TILE.player) {
		nextScore += 1;
		nextMap[nr]![nc] = TILE.player;
	} else {
		nextMap[nr]![nc] = TILE.deadBySuicide;
	}

	for (const row of nextMap) {
		if (row.includes(TILE.deadByHole) || row.includes(TILE.deadBySuicide)) {
			playerDead = true;
			levelComplete = false;
			break;
		}
	}
	if (!playerDead) {
		levelComplete = !nextMap.some((row) => row.includes(TILE.unvisited));
	}

	return Object.freeze({
		map: nextMap,
		score: nextScore,
		player_dead: playerDead,
		level_complete: levelComplete,
		status: statusOf(playerDead, levelComplete),
		map_json: JSON.stringify(nextMap)
	});
}

/** Parse a `map_json` board; throws if shape is not `number[][]`. */
export function parseBoard(mapJson: string): number[][] {
	const value = JSON.parse(mapJson || '[]') as unknown;
	if (
		!Array.isArray(value) ||
		!value.every(
			(row) => Array.isArray(row) && row.every((cell) => typeof cell === 'number')
		)
	) {
		throw new SimulateMoveError('invalid map_json');
	}
	return value as number[][];
}
