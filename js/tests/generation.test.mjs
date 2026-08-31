import assert from 'node:assert/strict';
import test from 'node:test';

import { parseDistributedGenerationEnvelope } from '../dist/index.js';

const hash = (byte) => `sha256:${byte.repeat(64)}`;

test('generation envelope keeps only bounded comparable identities', () => {
	const parsed = parseDistributedGenerationEnvelope({
		version: 1,
		generationId: hash('a'),
		releaseId: hash('b'),
		schemaId: hash('c')
	});
	assert.deepEqual(parsed, {
		version: 1,
		generationId: hash('a'),
		releaseId: hash('b'),
		schemaId: hash('c')
	});
	assert.ok(Object.isFrozen(parsed));
});

test('generation envelope rejects unsupported, missing, and unbounded values', () => {
	assert.throws(() => parseDistributedGenerationEnvelope({ version: 2 }));
	assert.throws(() =>
		parseDistributedGenerationEnvelope({
			version: 1,
			generationId: '',
			releaseId: hash('b')
		})
	);
	assert.throws(() =>
		parseDistributedGenerationEnvelope({
			version: 1,
			generationId: 'x'.repeat(513),
			releaseId: hash('b')
		})
	);
});
