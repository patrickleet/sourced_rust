<script lang="ts">
	import '../app.css';
	import '$lib/styles/chrome.css';
	import { browser } from '$app/environment';
	import { onDestroy, untrack } from 'svelte';
	import type { Snippet } from 'svelte';
	import {
		createPageDataSessionSource,
		matchDistributedRoute,
		type SveltekitReplicaHydration
	} from '@hops-ops/distributed/sveltekit';
	import { DISTRIBUTED_ROUTE_OPERATIONS, provideDistributed } from '$distributed';
	import { CHAT_PAGE_SIZE } from '$lib/chat/lobby-log';

	import AuthRefresh from '$lib/components/shared/AuthRefresh.svelte';
	import Navbar from '$lib/components/shared/header/Navbar.svelte';

	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const initialData = untrack(() => data);
	const pageData = createPageDataSessionSource(initialData);
	let appliedHydration: SveltekitReplicaHydration | undefined =
		initialData.distributed;
	let hydrationTimer: ReturnType<typeof setTimeout> | undefined;

	const client = provideDistributed({
		session: pageData.session,
		browser,
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

	onDestroy(() => {
		if (hydrationTimer !== undefined) clearTimeout(hydrationTimer);
		client.destroy();
	});

	$effect(() => {
		if (!browser) return;
		const onEnter = (event: PointerEvent) => {
			const node = event.target;
			if (!(node instanceof Element)) return;
			const link = node.closest('a[href]');
			if (!(link instanceof HTMLAnchorElement)) return;
			if (link.target && link.target !== '_self') return;
			if (link.origin !== window.location.origin) return;
			const signedIn = !!data.session?.user;
			// Anonymous routes install their own public-surface client below this
			// layout. Prefetching their user-surface artifact here cannot warm that
			// client and may establish the wrong schema binding before navigation.
			if (!signedIn) return;
			for (const { plan, artifact } of DISTRIBUTED_ROUTE_OPERATIONS) {
				if (!matchDistributedRoute(plan.route, link.pathname)) continue;
				const variables =
					plan.operation === 'ChatMessages'
						? { limit: CHAT_PAGE_SIZE, offset: 0 }
						: {};
				void client.prefetch(artifact, variables);
			}
		};
		document.addEventListener('pointerenter', onEnter, true);
		return () => document.removeEventListener('pointerenter', onEnter, true);
	});
</script>

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
