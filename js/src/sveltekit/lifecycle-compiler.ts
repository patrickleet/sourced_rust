import { realpathSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

import {
	generateDistributedSvelteKitLifecycle,
	type DistributedSvelteKitViteOptions
} from './vite.js';

function requiredEnvironment(name: string): string {
	const value = process.env[name];
	if (value === undefined || !isAbsolute(value)) {
		throw new TypeError(`${name} must be an absolute path`);
	}
	return realpathSync(resolve(value));
}

async function main(): Promise<void> {
	const [configPath, ...unexpected] = process.argv.slice(2);
	if (configPath === undefined || unexpected.length > 0 || !isAbsolute(configPath)) {
		throw new TypeError('usage: lifecycle-compiler <absolute distributed.config.js>');
	}
	const loaded = await import(pathToFileURL(realpathSync(configPath)).href);
	const options = loaded.distributedViteOptions as
		| DistributedSvelteKitViteOptions
		| undefined;
	if (options === undefined) {
		throw new TypeError(
			`${configPath} must export distributedViteOptions for lifecycle-owned client generation`
		);
	}
	await generateDistributedSvelteKitLifecycle(options, {
		projectRoot: requiredEnvironment('DISTRIBUTED_LIFECYCLE_ROOT'),
		stage: requiredEnvironment('DISTRIBUTED_LIFECYCLE_STAGE')
	});
}

await main().catch((error: unknown) => {
	const message = error instanceof Error ? (error.stack ?? error.message) : String(error);
	process.stderr.write(`${message}\n`);
	process.exitCode = 1;
});
