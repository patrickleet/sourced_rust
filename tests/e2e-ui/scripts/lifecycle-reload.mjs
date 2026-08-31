import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const application = resolve(root, 'crates/todo-domain/src/commands/force_archive.rs');
const framework = resolve(root, '../../js/src/sveltekit/lifecycle.ts');
const adminCommands = resolve(root, 'ui/src/lib/generated/admin/commands.ts');
const lifecycleFile = resolve(root, '.distributed/lifecycle/dev.json');
const original = readFileSync(application, 'utf8');
const frameworkOriginal = readFileSync(framework, 'utf8');
const baseURL = process.env.E2E_UI_ORIGIN || 'http://127.0.0.1:5180';
const apiURL = process.env.E2E_API_ORIGIN || 'http://127.0.0.1:8791';
const timeoutMs = 120_000;
const preparingEvents = [];

async function waitFor(predicate, label, timeout = timeoutMs) {
	const started = Date.now();
	while (Date.now() - started < timeout) {
		const value = await predicate();
		if (value) return value;
		await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
	}
	throw new Error(`timed out waiting for ${label}`);
}

async function lifecycleState() {
	try {
		return JSON.parse(readFileSync(lifecycleFile, 'utf8'));
	} catch {
		return undefined;
	}
}

async function transition(page, path, source, expectedReplicaRestore, assertGate = false) {
	const before = await lifecycleState();
	assert.equal(before.phase, 'active');
	await page.evaluate(() => {
		globalThis.__distributedReloadEvents = [];
	});
	preparingEvents.length = 0;
	const navigations = [];
	const onNavigation = (frame) => {
		if (frame === page.mainFrame()) navigations.push(frame.url());
	};
	page.on('framenavigated', onNavigation);
	let mutationRequests = 0;
	const documentRequests = [];
	const onRequest = (request) => {
		if (request.resourceType() === 'document') documentRequests.push(request.url());
		if ((request.postData() ?? '').includes('todos_create')) mutationRequests += 1;
	};
	page.on('request', onRequest);
	writeFileSync(path, source);

	await waitFor(
		async () => (await lifecycleState())?.phase === 'preparing',
		'pending lifecycle generation'
	);
	if (assertGate) {
		await waitFor(() => preparingEvents.length > 0, 'browser command gate');
		await page.locator('#todo-title').fill(`must-not-dispatch-${Date.now()}`);
		await page.getByRole('button', { name: /^add$/i }).click();
		await page.getByText(/paused during a coherent application reload/i).waitFor();
		assert.equal(mutationRequests, 0, 'reload gate must reject before GraphQL dispatch');
		const readable = await fetch(`${apiURL}/graphql`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ query: 'query ReloadReadProbe { __typename }' })
		});
		assert.equal(readable.status, 200, 'pending API must continue serving GraphQL reads');
		const direct = await fetch(`${apiURL}/graphql`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ query: 'mutation ReloadGateProbe { __typename }' })
		});
		assert.equal(direct.status, 503, 'pending API must reject direct GraphQL dispatch');
		assert.equal(
			(await direct.json()).errors[0].extensions.code,
			'APPLICATION_RELOADING'
		);
	}

	const restored = await waitFor(
		async () => {
			if (page.isClosed()) return undefined;
			return page.evaluate(() => {
				const state = globalThis;
				return state.__distributedReloadEvents?.at(-1);
			}).catch(() => undefined);
		},
		'controlled browser reload restoration'
	);
	assert.equal(restored.replicaCaptured, true, 'authenticated replica must participate');
	assert.equal(restored.replicaRestored, expectedReplicaRestore);
	assert.equal(
		documentRequests.length,
		1,
		`coherent transition must request exactly one document: frames=${JSON.stringify(navigations)} documents=${JSON.stringify(documentRequests)}`
	);
	assert.equal(new URL(page.url()).hash, '#reload-proof');
	assert.equal(
		await page.evaluate(() => globalThis.__distributedReloadState?.value),
		'preserve-me'
	);
	const after = await lifecycleState();
	assert.equal(after.phase, 'active');
	assert.notEqual(after.active.generationId, before.active.generationId);
	if (expectedReplicaRestore) {
		assert.equal(after.active.compatibilityId, before.active.compatibilityId);
	} else {
		assert.notEqual(after.active.compatibilityId, before.active.compatibilityId);
	}
	page.off('framenavigated', onNavigation);
	page.off('request', onRequest);
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
	storageState: resolve(root, 'e2e/.auth/alice.json')
});
await context.addInitScript(() => {
	globalThis.__distributedReloadEvents = [];
	addEventListener('distributed:reload-restored', (event) => {
		globalThis.__distributedReloadEvents.push(event.detail);
	});
	addEventListener('distributed:reload-preparing', (event) => {
		void globalThis.__distributedRecordReloadPreparing(event.detail);
	});
});
const page = await context.newPage();
await page.exposeFunction('__distributedRecordReloadPreparing', (detail) => {
	preparingEvents.push(detail);
});
page.on('console', (message) => {
	if (message.type() === 'error' || message.text().includes('Distributed reload')) {
		console.error(`browser console ${message.type()}: ${message.text()}`);
	}
});
page.on('pageerror', (error) => console.error(`browser page error: ${error.message}`));
const baseline = await lifecycleState();
assert.equal(baseline?.phase, 'active');
try {
	await page.goto(`${baseURL}/todos#reload-proof`, { waitUntil: 'domcontentloaded' });
	await page.getByRole('heading', { name: /todos/i }).waitFor();
	await waitFor(
		() => page.evaluate(() => globalThis.__distributedReloadState !== undefined),
		'client hydration'
	);
	await page.evaluate(() => {
		globalThis.__distributedReloadState.value = 'preserve-me';
	});

	const compatible = `${original}\n// lifecycle-compatible source-only rebuild\n`;
	await transition(page, application, compatible, true, true);

	const frameworkCompatible = `${frameworkOriginal}\n// lifecycle-compatible framework rebuild\n`;
	await transition(page, framework, frameworkCompatible, true);

	const incompatible = compatible.replace(
		'field: "todos_force_archive",',
		'field: "todos_force_archive_reload",'
	);
	assert.notEqual(incompatible, compatible, 'incompatible fixture edit must apply');
	await transition(page, application, incompatible, false);
	console.log('lifecycle-reload: application + framework + incompatible transitions OK');
} finally {
	writeFileSync(application, original);
	writeFileSync(framework, frameworkOriginal);
	await waitFor(async () => {
		const state = await lifecycleState();
		return state?.phase === 'active' &&
			state.active.generationId === baseline.active.generationId &&
			!readFileSync(adminCommands, 'utf8').includes('todos_force_archive_reload');
	}, 'baseline source and generated-client restoration');
	await context.close();
	await browser.close();
}
