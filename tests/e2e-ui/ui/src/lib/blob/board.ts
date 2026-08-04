/**
 * Board JSON helpers for rendering query data. Not game rules — the domain
 * owns move outcomes on the server.
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

export class BoardParseError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'BoardParseError';
	}
}

/** Parse a `map_json` board from the read model. */
export function parseBoard(mapJson: string): number[][] {
	const value = JSON.parse(mapJson || '[]') as unknown;
	if (
		!Array.isArray(value) ||
		!value.every(
			(row) => Array.isArray(row) && row.every((cell) => typeof cell === 'number')
		)
	) {
		throw new BoardParseError('invalid map_json');
	}
	return value as number[][];
}
