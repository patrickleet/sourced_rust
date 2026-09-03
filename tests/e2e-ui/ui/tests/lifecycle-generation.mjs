import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const uiRoot = path.resolve(
	path.dirname(fileURLToPath(import.meta.url)),
	'..'
);
const projectRoot = path.resolve(uiRoot, '..');
const lifecycleRoot = path.resolve(
	process.env.DISTRIBUTED_LIFECYCLE_DIR ??
		path.join(projectRoot, '.distributed/lifecycle')
);
const pointer = JSON.parse(
	fs.readFileSync(path.join(lifecycleRoot, 'active.json'), 'utf8')
);
if (!/^sha256:[0-9a-f]{64}$/.test(pointer.generation_id)) {
	throw new Error('active Distributed lifecycle generation is invalid');
}
const generationRoot = path.join(
	lifecycleRoot,
	'generations',
	pointer.generation_id
);
const generatedRoot = path.join(
	generationRoot,
	path.relative(projectRoot, uiRoot),
	'src/lib/generated'
);

export const generatedPath = (...segments) =>
	path.join(generatedRoot, ...segments);

export const readGenerated = (...segments) =>
	fs.readFileSync(generatedPath(...segments), 'utf8');
