import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const uiRoot = dirname(fileURLToPath(import.meta.url));
const e2eRoot = resolve(uiRoot, '..');
const distributedRoot = resolve(uiRoot, '../../..');

const manifestArgs = [
	'client-manifest',
	'--manifest-path',
	resolve(e2eRoot, 'Cargo.toml'),
	'--package',
	'e2e-service',
	'--distributed-path',
	distributedRoot
];

/** App-owned service/surface configuration; all generated behavior is package-owned. */
export const distributedClients = Object.freeze([
	Object.freeze({
		module: '$distributed',
		manifest: Object.freeze({ args: Object.freeze(manifestArgs) }),
		surface: 'fieldnote',
		documents: Object.freeze([
			'src/routes/todos/+page.graphql',
			'src/routes/chat/+page.graphql',
			'src/routes/blob/*/+page.graphql'
		]),
		out: 'src/lib/generated/user'
	}),
	Object.freeze({
		module: '$distributed/admin',
		manifest: Object.freeze({
			args: Object.freeze([
				...manifestArgs,
				'--entrypoint',
				'e2e_service::distributed_admin_client_surface'
			])
		}),
		surface: 'fieldnote-admin',
		documents: Object.freeze(['src/routes/admin/+page.graphql']),
		out: 'src/lib/generated/admin'
	})
]);

export const distributedViteOptions = Object.freeze({
	cwd: uiRoot,
	command: 'cargo',
	commandArgs: Object.freeze([
		'run',
		'--quiet',
		'--manifest-path',
		resolve(distributedRoot, 'Cargo.toml'),
		'-p',
		'distributed_cli',
		'--bin',
		'dctl',
		'--'
	]),
	clients: distributedClients
});
