<script lang="ts">
	import '../app.css';
	import '$lib/styles/chrome.css';
	import { browser } from '$app/environment';
	import { page } from '$app/state';
	import { onDestroy, untrack } from 'svelte';
	import type { Snippet } from 'svelte';
	import {
		createPageDataSessionSource,
		type SveltekitReplicaHydration
	} from '@hops-ops/distributed/sveltekit';
	import { DISTRIBUTED_BOUNDARY_OPERATIONS, provideDistributed } from '$distributed';

	import AuthRefresh from '$lib/components/shared/AuthRefresh.svelte';
	import Navbar from '$lib/components/shared/header/Navbar.svelte';

	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const initialData = untrack(() => data);
	const pageData = createPageDataSessionSource(initialData);
	let appliedHydration: SveltekitReplicaHydration | undefined =
		initialData.distributed;
	let hydrationTimer: ReturnType<typeof setTimeout> | undefined;
	const lifecycleDemoState: { value: string } =
		browser && (globalThis as typeof globalThis & Record<string, unknown>)
			.__distributedReloadState !== undefined
			? (globalThis as typeof globalThis & Record<string, unknown>)
					.__distributedReloadState as { value: string }
			: { value: 'initial' };
	if (browser) {
		const diagnostics = globalThis as typeof globalThis & Record<string, unknown>;
		diagnostics.__distributedReloadState = lifecycleDemoState;
	}

	const client = provideDistributed({
		boundaries: DISTRIBUTED_BOUNDARY_OPERATIONS,
		session: pageData.session,
		browser,
		reload: {
			state: [
				{
					key: 'e2e-ui.lifecycle-demo',
					fingerprint: 'v1',
					capture: () => lifecycleDemoState.value,
					restore: (value) => {
						if (typeof value === 'string') lifecycleDemoState.value = value;
					}
				}
			]
		},
		...(initialData.distributed !== undefined &&
		initialData.distributedAuthority !== undefined
			? {
					hydration: initialData.distributed,
					authority: initialData.distributedAuthority
				}
			: {})
	});

	$effect(() => {
		pageData.set(data);
		if (
			data.distributed === undefined ||
			data.distributedAuthority === undefined ||
			data.distributed === appliedHydration
		) {
			return;
		}

		appliedHydration = data.distributed;
		if (hydrationTimer !== undefined) clearTimeout(hydrationTimer);
		// Session listeners fence an old credential in the microtask queue.
		// Apply the separately-authorized navigation seed after that fence.
		// Same-scope hydrate merges into the warm replica (framework policy):
		// confirmed keys omitted from this route seed are retained.
		hydrationTimer = setTimeout(() => {
			hydrationTimer = undefined;
			client.hydrate(data.distributed!, data.distributedAuthority!);
		}, 0);
	});

	$effect(() => {
		if (!browser || !data.session?.user) return;
		const retention = client.retainLocation(
			{
				id: 'root-active-page',
				pathname: page.url.pathname,
				kind: 'page'
			},
			{
				search: page.url.searchParams,
				session: data.session,
				props: data
			}
		);
		return () => retention.release();
	});

	function prefetchLink(event: PointerEvent) {
		if (!browser || !data.session?.user) return;
		const target = event.target;
		if (!(target instanceof Element)) return;
		const anchor = target.closest('a[href]');
		if (!(anchor instanceof HTMLAnchorElement)) return;
		const targetUrl = new URL(anchor.href, page.url);
		if (targetUrl.origin !== page.url.origin) return;
		void client.prefetchLocation(targetUrl.pathname, {
			search: targetUrl.searchParams,
			session: data.session,
			props: data
		}).catch(() => undefined);
	}

	onDestroy(() => {
		if (hydrationTimer !== undefined) clearTimeout(hydrationTimer);
		client.destroy();
	});

</script>

<svelte:window onpointerover={prefetchLink} />
<AuthRefresh />
<Navbar />
<main>
	{@render children()}
</main>

<style>
	main {
		position: relative;
		min-height: 100vh;
	}
</style>
