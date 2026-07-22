import { gzipSync } from 'node:zlib';
import { build } from 'esbuild';

const packageRoot = new URL('../', import.meta.url).pathname;

async function bundle(contents, sourcefile) {
	const result = await build({
		stdin: {
			contents,
			loader: 'ts',
			resolveDir: packageRoot,
			sourcefile
		},
		bundle: true,
		format: 'esm',
		platform: 'browser',
		target: ['es2022'],
		minify: true,
		metafile: true,
		write: false,
		logLevel: 'silent'
	});
	const output = result.outputFiles[0].contents;
	return {
		minifiedBytes: output.byteLength,
		minifiedGzipBytes: gzipSync(output, { level: 9 }).byteLength,
		moduleCount: Object.keys(result.metafile.inputs).length
	};
}

const baseline = await bundle(`export const marker = 1;`, 'baseline.ts');
const purposeBuilt = await bundle(
	`export { createCacheEngine, cacheIndexKey } from './src/internal/cache-engine.ts';`,
	'purpose-built.ts'
);
const apollo = await bundle(
	`export { InMemoryCache } from '@apollo/client/cache';`,
	'apollo.ts'
);
const unusedPurposeBuilt = await bundle(
	`import { createCacheEngine } from './src/internal/cache-engine.ts'; export const marker = 1;`,
	'unused-purpose-built.ts'
);
const unusedApollo = await bundle(
	`import { InMemoryCache } from '@apollo/client/cache'; export const marker = 1;`,
	'unused-apollo.ts'
);

const report = {
	measurement: 'esbuild browser ESM, ES2022, minified; gzip level 9',
	versions: {
		esbuild: (await import('esbuild/package.json', { with: { type: 'json' } })).default.version,
		apolloClient: (await import('@apollo/client/package.json', { with: { type: 'json' } })).default
			.version
	},
	baseline,
	candidates: {
		purposeBuilt,
		apollo
	},
	unusedImports: {
		purposeBuilt: unusedPurposeBuilt,
		apollo: unusedApollo,
		matchesBaseline:
			unusedPurposeBuilt.minifiedBytes === baseline.minifiedBytes &&
			unusedApollo.minifiedBytes === baseline.minifiedBytes
	}
};

process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
