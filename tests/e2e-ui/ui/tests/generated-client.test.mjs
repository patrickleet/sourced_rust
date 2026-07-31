import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const fixtureRoot = path.resolve(uiRoot, '..');
const read = (...segments) =>
	fs.readFileSync(path.join(uiRoot, ...segments), 'utf8');

test('co-located operations are the only app-authored GraphQL documents', () => {
	const documents = [
		['src/routes/todos/+page.graphql', /query Todos @load/],
		['src/routes/chat/+page.graphql', /query ChatMessages\(\$limit: Int!, \$offset: Int!\) @load @live/],
		['src/routes/blob/[[gameId]]/+page.graphql', /query BlobGames @load/],
		['src/routes/admin/+page.graphql', /query AdminAllTodos @load/]
	];
	for (const [relative, declaration] of documents) {
		assert.match(read(relative), declaration, relative);
	}

	const authored = fs
		.readdirSync(path.join(uiRoot, 'src/routes'), { recursive: true })
		.filter((entry) => typeof entry === 'string' && /\.(gql|graphql)$/.test(entry))
		.sort();
	assert.deepEqual(authored, [
		'admin/+page.graphql',
		'blob/[[gameId]]/+page.graphql',
		'chat/+page.graphql',
		'todos/+page.graphql'
	]);
});

test('generated route registries bind static @load ownership to artifacts', () => {
	const userRoutes = read('src/lib/generated/user/routes.ts');
	assert.match(userRoutes, /Operation_Todos/);
	assert.match(userRoutes, /Operation_ChatMessages/);
	assert.match(userRoutes, /Operation_BlobGames/);
	assert.ok(userRoutes.includes('"route": "/todos"'));
	assert.ok(userRoutes.includes('"route": "/chat"'));
	assert.ok(userRoutes.includes('"route": "/blob/[[gameId]]"'));
	assert.match(userRoutes, /DISTRIBUTED_ROUTE_OPERATIONS/);
	assert.doesNotMatch(userRoutes, /AdminAllTodos/);

	const adminRoutes = read('src/lib/generated/admin/routes.ts');
	assert.match(adminRoutes, /Operation_AdminAllTodos/);
	assert.ok(adminRoutes.includes('"route": "/admin"'));
	assert.doesNotMatch(adminRoutes, /Operation_Todos/);
});

test('normal and elevated command inventories cannot be mixed', () => {
	const user = read('src/lib/generated/user/commands.ts');
	const admin = read('src/lib/generated/admin/commands.ts');

	assert.match(user, /"name": "todo\.create"/);
	assert.match(user, /"name": "chat\.post"/);
	assert.match(user, /"name": "blob\.start"/);
	assert.match(user, /"name": "blob\.move"/);
	assert.doesNotMatch(user, /"name": "todo\.force_archive"/);
	assert.match(user, /"kind": "application"/);
	assert.match(user, /"name": "e2e-ui"/);
	// Eligible roles include admin so multi-role principals can open the portable
	// user schema without a separate downloadable bundle.
	assert.match(user, /"roles": \[\s*"admin",\s*"user"\s*\]/);

	assert.match(admin, /"name": "todo\.force_archive"/);
	assert.match(admin, /"name": "e2e-ui-admin"/);
	assert.match(admin, /"roles": \[\s*"admin"\s*\]/);
});

test('SvelteKit composition uses one generated replica and a nested elevated boundary', () => {
	const rootServer = read('src/routes/+layout.server.ts');
	const rootLayout = read('src/routes/+layout.svelte');
	const adminServer = read('src/routes/admin/+layout.server.ts');
	const adminLayout = read('src/routes/admin/+layout.svelte');

	assert.match(rootServer, /createDistributedSvelteKitServer/);
	assert.match(rootServer, /DISTRIBUTED_ROUTE_OPERATIONS/);
	assert.match(rootServer, /from '\$distributed'/);
	assert.match(rootLayout, /createPageDataSessionSource/);
	assert.match(rootLayout, /provideDistributed/);
	assert.match(rootLayout, /from '\$distributed'/);
	assert.doesNotMatch(rootLayout, /\$lib\/distributed/);

	assert.match(adminServer, /isAdminEngineRole/);
	assert.match(adminServer, /error\(\s*403/);
	assert.match(adminServer, /from '\$distributed\/admin'/);
	assert.match(adminLayout, /from '\$distributed\/admin'/);
	assert.match(adminLayout, /provideDistributed/);

	const localHelpers = path.join(uiRoot, 'src/lib/distributed');
	assert.deepEqual(
		fs.existsSync(localHelpers) ? fs.readdirSync(localHelpers) : [],
		[],
		'the fixture must not retain generic Svelte context/session helpers'
	);
});

test('pages consume generated operation state and causal commands only', () => {
	const expectations = [
		[
			'src/routes/todos/+page.svelte',
			/Todos\.use\(\)/,
			/commands\.todo\.create/
		],
		[
			'src/routes/chat/+page.svelte',
			/ChatMessages\.use\(/,
			/commands\.chat\.post/
		],
		[
			'src/routes/blob/[[gameId]]/+page.svelte',
			/BlobGames\.use\(\)/,
			/commands\.blob\.move/
		],
		[
			'src/routes/admin/+page.svelte',
			/AdminAllTodos\.use\(\)/,
			/commands\.todo\.force_archive/
		]
	];

	for (const [relative, operation, command] of expectations) {
		const source = read(relative);
		assert.match(source, operation, relative);
		assert.match(source, command, relative);
		assert.doesNotMatch(
			source,
			/useGraphql|useDistributedClient|gql\.(?:store|live)|list\.(?:seed|target|scheduleCatchUp)|optimistic:\s*\{|rememberedRows|seedList|applyRow|optimisticMove|pendingConfirms|lastConfirmed|sort(?:Admin)?Todos|sortChatMessages/,
			relative
		);
		assert.doesNotMatch(source, /\.resource|commands\.generated/, relative);
	}
});

test('chat loads a fixed page of 25 with infinite history helpers', () => {
	const page = read('src/routes/chat/+page.svelte');
	const layout = read('src/routes/+layout.server.ts');
	const helpers = read('src/lib/chat/lobby-log.ts');

	assert.match(helpers, /export const CHAT_PAGE_SIZE = 25/);
	assert.match(page, /CHAT_PAGE_SIZE/);
	assert.match(page, /mergeHistoryPage/);
	assert.match(page, /loadOlder/);
	assert.match(page, /chat-load-earlier/);
	assert.match(page, /column-reverse/);
	assert.match(layout, /CHAT_PAGE_SIZE/);
	assert.match(layout, /ChatMessages:\s*\(\)\s*=>\s*\(\{\s*limit:\s*CHAT_PAGE_SIZE,\s*offset:\s*0\s*\}\)/);
	assert.doesNotMatch(page, /PAGE_SIZE = 40|CHAT_PAGE_SIZE = 40/);
	assert.doesNotMatch(layout, /CHAT_PAGE_SIZE = 40/);
});

test('Vite owns user/admin compiler entrypoints and generated modules stay state-free', () => {
	const config = read('distributed.config.js');
	const vite = read('vite.config.ts');
	const svelte = read('svelte.config.js');
	const user = read('src/lib/generated/user/sveltekit.ts');
	const admin = read('src/lib/generated/admin/sveltekit.ts');

	assert.match(config, /module: '\$distributed'/);
	assert.match(config, /module: '\$distributed\/admin'/);
	assert.match(config, /client-manifest/);
	assert.match(config, /distributed_admin_client_surface/);
	assert.match(vite, /distributedSvelteKit\(distributedViteOptions\)/);
	assert.match(svelte, /distributedSvelteKitAliases/);

	for (const generated of [user, admin]) {
		assert.match(generated, /defineDistributedSvelteKitOperation/);
		assert.match(generated, /export function provideDistributed/);
		assert.match(generated, /export function useCommands/);
		assert.doesNotMatch(
			generated,
			/^(?:const|let|var) (?:client|commands)\b/m,
			'generated module must retain only static artifacts'
		);
	}
});

test('fixture generation is one dctl pipeline over typed Service inventory', () => {
	const makefile = fs.readFileSync(path.join(fixtureRoot, 'Makefile'), 'utf8');
	const config = read('distributed.config.js');
	const runner = read('scripts/distributed-client.mjs');
	const service = fs.readFileSync(
		path.join(fixtureRoot, 'crates/service/src/service.rs'),
		'utf8'
	);

	assert.match(makefile, /^gen-client:/m);
	assert.match(makefile, /^check-client:/m);
	assert.match(makefile, /client:generate/);
	assert.match(makefile, /client:check/);
	assert.doesNotMatch(
		makefile,
		/client-manifest|--documents|--surface|src\/lib\/generated\/(?:user|admin)/,
		'Make must not duplicate distributed.config.js'
	);
	assert.match(config, /client-manifest/);
	assert.match(config, /surface: 'e2e-ui'/);
	assert.match(config, /surface: 'e2e-ui-admin'/);
	assert.match(runner, /generateDistributedSvelteKit/);
	assert.match(runner, /checkDistributedSvelteKit/);
	assert.doesNotMatch(
		makefile,
		/export-commands|gen-commands|check-commands|gen-gql|check-gql/
	);

	assert.match(service, /\.typed_command\(/);
	assert.match(service, /distributed_client_surface/);
	assert.match(service, /distributed_admin_client_surface/);
	// Multi-role: eligible {admin,user} + schema privilege {user} via contract API.
	assert.match(service, /surface_for_application_contract\(/);
	assert.match(service, /client_application_surface_with_schema_roles\(/);
	assert.match(service, /&?\["admin", "user"\]/);
	assert.match(service, /&?\["user"\]/);
	assert.match(service, /default input\.todo_id = uuid_v7\(\)/);
	assert.doesNotMatch(service, /pub fn graphql_commands/);
});
