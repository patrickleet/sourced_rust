import { sveltekit } from '@sveltejs/kit/vite';
import {
	distributedGraphqlProxy,
	distributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';
import { defineConfig } from 'vite';

import { distributedViteOptions } from './distributed.config.js';

const api = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL || 'http://127.0.0.1:8791';
const uiPort = Number(process.env.UI_PORT || '5180');
if (!Number.isSafeInteger(uiPort) || uiPort < 1 || uiPort > 65_535) {
	throw new TypeError('UI_PORT must be an integer from 1 through 65535');
}

export default defineConfig({
	plugins: [
		distributedSvelteKit(distributedViteOptions),
		sveltekit()
	],
	css: { devSourcemap: true },
	// blob-domain pure package (wasm-pack --features wasm)
	assetsInclude: ['**/*.wasm'],
	server: {
		port: uiPort,
		// hops local cluster-DNS mode uses svc.ns.svc.cluster.local Host headers.
		host: true,
		allowedHosts: true,
		// GraphQL-only public API (commands are mutations, not POST /todo.*).
		proxy: distributedGraphqlProxy(api)
	},
	optimizeDeps: {
		exclude: ['$lib/blob/pkg/blob_wasm.js']
	}
});
