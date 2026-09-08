import { test, expect } from '@playwright/test';
import { build } from '../../../js/node_modules/esbuild/lib/main.js';
import { createDistributedReplica } from '../../../js/dist/replica/index.js';
import { artifact, frame } from '../../../js/tests/fixtures/unique-key-artifact.mjs';
import { fileURLToPath } from 'node:url';

// Browser runtime contract proof. The transport is controlled; SQL joins and
// application SSR are covered separately, not simulated by this test.
test('candidate-key artifact hydrates and changes relationships in Chromium', async ({ page }) => {
	const server = createDistributedReplica();
	server.writeResult(artifact, {}, frame('1', 'target-one', 'first'), 'network');
	const initial = server.read(artifact, {});
	const seed = server.dehydrate();
	const runtimePath = fileURLToPath(new URL('../../../js/dist/replica/index.js', import.meta.url));
	const bundle = await build({
		stdin: { contents: `
			import { createDistributedReplica } from ${JSON.stringify(runtimePath)};
			globalThis.startUniqueKeyProof = (artifact, seed) => {
				let observer;
				const state = { fetches: 0, subscriptions: 0, closed: false };
				const replica = createDistributedReplica({ transport: {
					fetch() { state.fetches++; throw new Error('unexpected hydration fetch'); },
					subscribe(request, next) {
						state.subscriptions++; observer = next;
						return () => { state.closed = true; };
					}
				} });
				if (!replica.hydrate(seed, seed.scope)) throw new Error('hydration rejected');
				const watch = replica.watch(artifact, {}, { live: true });
				const unsubscribe = watch.subscribe(snapshot => {
					document.querySelector('output').textContent = JSON.stringify(snapshot.data);
				});
				return { state, update(frame) { observer.next(frame); },
					close() { unsubscribe(); watch.destroy(); } };
			};
		`, resolveDir: process.cwd(), loader: 'js' },
		bundle: true, write: false, platform: 'browser', format: 'iife'
	});
	await page.setContent('<output aria-label="Query result"></output>');
	await page.getByLabel('Query result').evaluate((element, data) => {
		element.textContent = JSON.stringify(data);
	}, initial.data);
	await expect(page.getByLabel('Query result')).toContainText('target-one');
	await page.addScriptTag({ content: bundle.outputFiles[0].text });
	await page.evaluate(({ artifact, seed }) => {
		(globalThis as any).proof = (globalThis as any).startUniqueKeyProof(artifact, seed);
	}, { artifact, seed });
	await expect(page.getByLabel('Query result')).toHaveText(JSON.stringify(initial.data));
	expect(await page.evaluate(() => (globalThis as any).proof.state)).toEqual({
		fetches: 0, subscriptions: 1, closed: false
	});
	await page.evaluate(value => (globalThis as any).proof.update(value), frame('2', 'target-two', 'second'));
	await expect(page.getByLabel('Query result')).toHaveText(JSON.stringify({ todos: [
		{ id: 'source-id', title: 'source', owner: { id: 'target-two', title: 'second' } }
	] }));
	await page.evaluate(value => (globalThis as any).proof.update(value), frame('3', null, null));
	await expect(page.getByLabel('Query result')).toHaveText(JSON.stringify({ todos: [
		{ id: 'source-id', title: 'source', owner: null }
	] }));
	await page.evaluate(() => (globalThis as any).proof.close());
	expect(await page.evaluate(() => (globalThis as any).proof.state)).toEqual({
		fetches: 0, subscriptions: 1, closed: true
	});
});
