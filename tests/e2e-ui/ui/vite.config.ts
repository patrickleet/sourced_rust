import { existsSync } from 'node:fs';
import { resolve } from 'node:path';

import { sveltekit } from '@sveltejs/kit/vite';
import {
	distributedGraphqlProxy,
	distributedLifecycle,
	distributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';
import { defineConfig } from 'vite';

import { distributedViteOptions } from './distributed.config.js';

const api = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL || 'http://127.0.0.1:8791';
const uiPort = Number(process.env.UI_PORT || '5180');
if (!Number.isSafeInteger(uiPort) || uiPort < 1 || uiPort > 65_535) {
	throw new TypeError('UI_PORT must be an integer from 1 through 65535');
}

// Cluster-dev node images often lack cargo/the distributed CLI. Prefer committed generated
// clients when present so vite can start without a Rust toolchain.
const generatedReady = distributedViteOptions.clients.every((client: { out: string }) =>
	existsSync(resolve(distributedViteOptions.cwd, client.out, 'sveltekit.ts'))
);
const skipClientCompile =
	process.env.DISTRIBUTED_SKIP_CLIENT_COMPILE === '1' ||
	(process.env.DISTRIBUTED_SKIP_CLIENT_COMPILE !== '0' &&
		process.env.DISTRIBUTED_LIFECYCLE_DIR === undefined &&
		generatedReady);

export default defineConfig({
	plugins: [
		...(skipClientCompile
			? [distributedLifecycle()]
			: [distributedSvelteKit(distributedViteOptions)]),
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
