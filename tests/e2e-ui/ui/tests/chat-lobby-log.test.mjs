/**
 * Unit tests for lobby chat log geometry + history merge.
 * Exercises the real `$lib/chat/lobby-log` module used by `/chat`.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const lobbyLogPath = path.join(uiRoot, 'src/lib/chat/lobby-log.ts');

const {
	CHAT_PAGE_SIZE,
	nearBottom,
	nearTop,
	needsHistoryFill,
	liveWindowProvesHistoryExhausted,
	mergeHistoryPage,
	pinScrollBottom,
	preserveScrollAfterPrepend,
	distanceFromBottom,
	distanceFromTop,
	scrollRange
} = await import(pathToFileURL(lobbyLogPath).href);

test('chat page size is 25 for SSR and client', () => {
	assert.equal(CHAT_PAGE_SIZE, 25);
});

test('column-reverse positive model: scrollTop 0 is newest; max is oldest', () => {
	assert.equal(pinScrollBottom(), 0);
	const bottom = { scrollTop: 0, scrollHeight: 800, clientHeight: 400 };
	assert.equal(distanceFromBottom(bottom), 0);
	assert.ok(nearBottom(bottom));
	assert.equal(nearTop(bottom), false);

	const top = { scrollTop: 400, scrollHeight: 800, clientHeight: 400 };
	assert.equal(distanceFromTop(top), 0);
	assert.equal(nearTop(top), true);
	assert.equal(nearBottom(top), false);
});

test('column-reverse Chromium model: negative scrollTop toward oldest', () => {
	// Matches Chromium: wheel "up" → scrollTop -100, min = -(scrollHeight-clientHeight).
	const bottom = { scrollTop: 0, scrollHeight: 1836, clientHeight: 200 };
	assert.equal(scrollRange(bottom), 1636);
	assert.equal(distanceFromBottom(bottom), 0);
	assert.ok(nearBottom(bottom));
	assert.equal(nearTop(bottom), false, 'must not load history at newest edge');

	const mid = { scrollTop: -100, scrollHeight: 1836, clientHeight: 200 };
	assert.equal(distanceFromBottom(mid), 100);
	assert.equal(
		nearBottom(mid),
		false,
		'negative scroll must detach stick-to-bottom (this was the scroll-up bug)'
	);
	assert.equal(nearTop(mid), false);

	const top = { scrollTop: -1636, scrollHeight: 1836, clientHeight: 200 };
	assert.equal(distanceFromTop(top), 0);
	assert.equal(nearTop(top), true);
	assert.equal(nearBottom(top), false);
});

test('no overflow is not nearTop (fill path uses needsHistoryFill instead)', () => {
	const fit = { scrollTop: 0, scrollHeight: 200, clientHeight: 400 };
	assert.equal(nearTop(fit), false);
	assert.equal(needsHistoryFill(fit), true);
	assert.equal(
		needsHistoryFill({ scrollTop: 0, scrollHeight: 800, clientHeight: 400 }),
		false
	);
});

test('a complete short live window proves there is no older page', () => {
	assert.equal(liveWindowProvesHistoryExhausted(0, 25), true);
	assert.equal(liveWindowProvesHistoryExhausted(24, 25), true);
	assert.equal(liveWindowProvesHistoryExhausted(25, 25), false);
});

test('mergeHistoryPage reverses desc server pages and advances offset', () => {
	const page = [
		{ message_id: 'm3', body: 'new' },
		{ message_id: 'm2', body: 'mid' },
		{ message_id: 'm1', body: 'old' }
	];
	const known = new Set(['m3']);
	const result = mergeHistoryPage(page, known, 25, 25);
	assert.deepEqual(
		result.fresh.map((m) => m.message_id),
		['m1', 'm2']
	);
	assert.equal(result.nextOffset, 50);
	assert.equal(result.hasMore, false);
});

test('mergeHistoryPage full window keeps hasMore true', () => {
	const page = Array.from({ length: 25 }, (_, i) => ({ message_id: `h${i}` }));
	const result = mergeHistoryPage(page, new Set(), 25, 25);
	assert.equal(result.fresh.length, 25);
	assert.equal(result.nextOffset, 50);
	assert.equal(result.hasMore, true);
});

test('mergeHistoryPage empty page ends history', () => {
	const result = mergeHistoryPage([], new Set(), 50, 25);
	assert.equal(result.fresh.length, 0);
	assert.equal(result.hasMore, false);
	assert.equal(result.nextOffset, 75);
});

test('preserveScrollAfterPrepend: positive model adds delta; negative keeps bottom-relative', () => {
	assert.equal(preserveScrollAfterPrepend(120, 500, 700), 320);
	assert.equal(preserveScrollAfterPrepend(0, 500, 500), 0);
	// Chromium reverse: stay at same bottom-relative offset when older rows prepend.
	assert.equal(preserveScrollAfterPrepend(-200, 800, 1000), -200);
});
