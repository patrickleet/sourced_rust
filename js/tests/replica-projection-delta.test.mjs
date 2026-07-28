import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import {
	canonicalCommandProjectionMetadata,
	canonicalProjectionDelta,
	parseCommandProjectionMetadata,
	parseProjectionDelta,
	validateProjectionMetadataAuthority
} from '../dist/replica/projection-delta/index.js';

const VECTOR_HASH =
	'7bdc06e1d3accc4c62132f967df1310d31f2f3b856fa7e97a7c5d0907a4ae17b';

const clone = (value) => structuredClone(value);

async function vector() {
	return JSON.parse(
		await readFile(
			new URL('../../tests/fixtures/projection-delta-v1.json', import.meta.url),
			'utf8'
		)
	);
}

async function metadataVector() {
	return JSON.parse(
		await readFile(
			new URL(
				'../../tests/fixtures/command-projection-metadata-v1.json',
				import.meta.url
			),
			'utf8'
		)
	);
}

test('Rust projection vector is byte-identical across every op and tagged value', async () => {
	const parsed = parseProjectionDelta(await vector());
	const canonical = canonicalProjectionDelta(parsed);
	assert.equal(Buffer.byteLength(canonical), 6327);
	assert.equal(
		createHash('sha256').update(canonical).digest('hex'),
		VECTOR_HASH
	);
	assert.equal(parsed.operations.length, 8);
	assert.deepEqual(
		parsed.operations.map(({ mutation }) => mutation.op),
		[
			'upsert',
			'patch',
			'delete',
			'link',
			'link',
			'unlink',
			'invalidate_model',
			'invalidate_relationship'
		]
	);
});

test('metadata validates identity, expiry, obligations, and canonical replay bytes', async () => {
	const parsed = parseCommandProjectionMetadata(await metadataVector());
	const projection = parsed.delta.projections[0];
	const contract = {
		version: 2,
		deltaWireVersion: 1,
		projectionProgramVersion: 2,
		operationSemanticsVersion: 1,
		projections: [
			{
				programId: projection.program_id,
				bindingId: projection.binding_id,
				epoch: projection.epoch,
				programIrVersion: 1,
				operationSemanticsVersion: 1
			}
		],
		eventSet: [],
		preview: { version: 1, occurrences: [], operations: [], recoveries: [] },
		fallback: 'revalidate'
	};
	const canonical = validateProjectionMetadataAuthority(
		parsed,
		contract,
		{
			surface: { kind: 'role', name: 'member' },
			schemaHash: 'sha256:schema',
			protocolHash: 'sha256:protocol',
			authorizationGeneration: 'auth-generation-1',
			cacheScope:
				'v1.cache-scope.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
			causationId: 'cause-1'
		},
		1_700_000_000_001
	);
	assert.equal(canonical, canonicalCommandProjectionMetadata(parsed));
	assert.throws(() =>
		validateProjectionMetadataAuthority(
			parsed,
			contract,
			{
				surface: { kind: 'role', name: 'member' },
				schemaHash: 'sha256:schema',
				protocolHash: 'sha256:protocol',
				authorizationGeneration: 'auth-generation-2',
				cacheScope:
					'v1.cache-scope.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
				causationId: 'cause-1'
			},
			1_700_000_000_001
		)
	);
	assert.throws(() =>
		validateProjectionMetadataAuthority(
			parsed,
			contract,
			{
				surface: { kind: 'role', name: 'member' },
				schemaHash: 'sha256:schema',
				protocolHash: 'sha256:protocol',
				cacheScope:
					'v1.cache-scope.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
				causationId: 'cause-1'
			},
			parsed.expiresAtUnixMs
		)
	);
});

test('strict hostile inputs fail before allocation or cache mutation', async () => {
	const original = await vector();
	const unknown = clone(original);
	unknown.identity.surprise = true;
	assert.throws(() => parseProjectionDelta(unknown), /identity\.surprise/);

	const reordered = clone(original);
	[reordered.operations[0], reordered.operations[1]] = [
		reordered.operations[1],
		reordered.operations[0]
	];
	assert.throws(() => parseProjectionDelta(reordered), /operations/);

	const oversizedKey = clone(original);
	oversizedKey.operations[0].mutation.scope.key[0].value.value = 'x'.repeat(4097);
	assert.throws(() => parseProjectionDelta(oversizedKey), /key/);

	const forgedToken = clone(original);
	forgedToken.identity.cache_scope_token =
		'v1.projection-partition.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
	assert.throws(() => parseProjectionDelta(forgedToken), /cache_scope_token/);

	const tooMany = clone(original);
	tooMany.occurrences = Array.from({ length: 129 }, (_, ordinal) => ({
		causation_id: original.identity.command_causation_id,
		ordinal,
		occurrence_id: `occurrence-${ordinal}`
	}));
	assert.throws(() => parseProjectionDelta(tooMany), /occurrences/);
});
