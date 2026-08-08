import { existsSync } from 'node:fs';
import { resolve } from 'node:path';

import { sveltekit } from '@sveltejs/kit/vite';
import {
	distributedGraphqlProxy,
	distributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';
import { defineConfig } from 'vite';

import { distributedViteOptions } from './distributed.config.js';

const api = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL || 'http://127.0.0.1:8791';

// Cluster-dev node images often lack cargo/dctl. Prefer committed generated
// clients when present so vite can start without a Rust toolchain.
const generatedReady = distributedViteOptions.clients.every((client) =>
	existsSync(resolve(distributedViteOptions.cwd, client.out, 'sveltekit.ts'))
);
const skipClientCompile =
	process.env.DISTRIBUTED_SKIP_CLIENT_COMPILE === '1' ||
	(process.env.DISTRIBUTED_SKIP_CLIENT_COMPILE !== '0' && generatedReady);

export default defineConfig({
	plugins: [
		...(skipClientCompile ? [] : [distributedSvelteKit(distributedViteOptions)]),
		sveltekit()
	],
	css: { devSourcemap: true },
	server: {
		port: 5180,
		// hops local cluster-DNS mode uses svc.ns.svc.cluster.local Host headers.
		host: true,
		allowedHosts: true,
		// GraphQL-only public API (commands are mutations, not POST /todo.*).
		proxy: distributedGraphqlProxy(api)
	}
});
