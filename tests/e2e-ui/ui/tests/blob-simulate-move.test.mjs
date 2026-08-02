/**
 * Parity with blob_domain::simulate_move (test_map_no_holes / hole cases).
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	simulateMove,
	TILE,
	SimulateMoveError
} from '../src/lib/blob/simulate-move.ts';

const noHoles = () => [
	[TILE.player, TILE.unvisited, TILE.unvisited],
	[TILE.unvisited, TILE.unvisited, TILE.unvisited],
	[TILE.unvisited, TILE.unvisited, TILE.unvisited]
];

const withHole = () => [
	[TILE.player, TILE.hole, TILE.unvisited],
	[TILE.unvisited, TILE.unvisited, TILE.unvisited],
	[TILE.unvisited, TILE.unvisited, TILE.unvisited]
];

test('right into unvisited visits previous and scores', () => {
	const preview = simulateMove(noHoles(), 0, 'right');
	assert.equal(preview.score, 1);
	assert.equal(preview.player_dead, false);
	assert.equal(preview.level_complete, false);
	assert.equal(preview.status, 'active');
	assert.equal(preview.map[0][0], TILE.visited);
	assert.equal(preview.map[0][1], TILE.player);
	assert.equal(JSON.parse(preview.map_json)[0][1], TILE.player);
});

test('right into hole dies without scoring', () => {
	const preview = simulateMove(withHole(), 0, 'right');
	assert.equal(preview.score, 0);
	assert.equal(preview.player_dead, true);
	assert.equal(preview.status, 'dead');
	assert.equal(preview.map[0][1], TILE.deadByHole);
});

test('edge move throws (server-local no-op path)', () => {
	assert.throws(() => simulateMove(noHoles(), 0, 'up'), SimulateMoveError);
	assert.throws(() => simulateMove(noHoles(), 0, 'left'), SimulateMoveError);
});

test('revisit visited cell is suicide', () => {
	const first = simulateMove(noHoles(), 0, 'right');
	const back = simulateMove(first.map, first.score, 'left');
	assert.equal(back.player_dead, true);
	assert.equal(back.map[0][0], TILE.deadBySuicide);
});
