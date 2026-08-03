import {
	checkDistributedSvelteKit,
	generateDistributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';

import { distributedViteOptions } from '../distributed.config.js';

const mode = process.argv[2];
if (mode === 'generate') {
	await generateDistributedSvelteKit(distributedViteOptions);
	console.log('Generated Distributed clients from distributed.clients.json');
} else if (mode === 'check') {
	await checkDistributedSvelteKit(distributedViteOptions);
	console.log('Distributed clients are current');
} else {
	throw new Error('usage: node scripts/distributed-client.mjs <generate|check>');
}
