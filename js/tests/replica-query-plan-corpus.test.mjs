import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import {
	compareReplicaOrder,
	evaluateReplicaFilter
} from '../dist/replica/index.js';

const corpus = JSON.parse(
	await readFile(
		new URL('../../tests/fixtures/client-query-plan-corpus.json', import.meta.url),
		'utf8'
	)
);

const literal = (value) => Object.freeze({ kind: 'literal', value });
const field = (name, scalar, codec) =>
	Object.freeze({
		field: name,
		scalar,
		codec,
		nullable: false,
		operators: Object.freeze([
			'_eq',
			'_neq',
			'_gt',
			'_gte',
			'_lt',
			'_lte',
			'_in',
			'_nin',
			'_is_null'
		])
	});
const fields = Object.freeze([
	field('id', 'ID', 'string'),
	field('priority', 'BigInt', 'json_number_precision_limited'),
	field('completed', 'Boolean', 'boolean')
]);

test('portable query plans match the shared SQLite server corpus', () => {
	for (const entry of corpus.cases) {
		const filter = Object.freeze({
			input: literal(entry.where),
			fields,
			relationships: Object.freeze([]),
			rowPolicy: Object.freeze({ kind: 'unrestricted' })
		});
		const order = Object.freeze({
			input: literal(entry.orderBy),
			fields: Object.freeze(
				fields.map(({ field, scalar, codec, nullable }) => ({
					field,
					scalar,
					codec,
					nullable
				}))
			),
			tieBreakers: Object.freeze([
				Object.freeze({
					field: 'id',
					scalar: 'ID',
					codec: 'string',
					nullable: false
				})
			])
		});
		const matched = corpus.records.filter((record) => {
			const evaluation = evaluateReplicaFilter(filter, record);
			assert.notEqual(evaluation.result, 'unknown', entry.name);
			return evaluation.result === 'match';
		});
		matched.sort((left, right) => {
			const comparison = compareReplicaOrder(order, left, right);
			assert.notEqual(comparison.result, 'unknown', entry.name);
			return comparison.result === 'less'
				? -1
				: comparison.result === 'greater'
					? 1
					: 0;
		});
		const actual = matched
			.slice(entry.offset, entry.offset + entry.limit)
			.map((record) => record.id);
		assert.deepEqual(actual, entry.expected, entry.name);
	}
});
