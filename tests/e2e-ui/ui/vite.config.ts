import { sveltekit } from '@sveltejs/kit/vite';
import {
	distributedGraphqlProxy,
	distributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';
import { defineConfig } from 'vite';

import { distributedViteOptions } from './distributed.config.js';

const api = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL || 'http://127.0.0.1:8791';

export default defineConfig({
	plugins: [distributedSvelteKit(distributedViteOptions), sveltekit()],
	css: { devSourcemap: true },
	server: {
		port: 5180,
		// GraphQL-only public API (commands are mutations, not POST /todo.*).
		proxy: distributedGraphqlProxy(api)
	}
});
