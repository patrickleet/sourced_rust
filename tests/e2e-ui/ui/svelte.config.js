import adapter from '@sveltejs/adapter-node';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { distributedSvelteKitAliases } from '@hops-ops/distributed/sveltekit/vite';

import {
	distributedClients,
	distributedViteOptions
} from './distributed.config.js';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		adapter: adapter({ out: 'build' }),
		alias: distributedSvelteKitAliases({
			cwd: distributedViteOptions.cwd,
			clients: distributedClients
		})
	}
};

export default config;
