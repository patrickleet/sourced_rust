<script lang="ts">
	/**
	 * When signed out, install the e2e-ui-public client so ChatMessages open with
	 * the anonymous surface. Signed-in traffic keeps the root portable user client.
	 */
	import { browser } from '$app/environment';
	import { page } from '$app/state';
	import { onDestroy, untrack } from 'svelte';
	import type { Snippet } from 'svelte';
	import {
		createPageDataSessionSource,
		useDistributedSvelteKitClient,
		type SveltekitReplicaHydration
	} from '@hops-ops/distributed/sveltekit';
	import { DISTRIBUTED_BOUNDARY_OPERATIONS, provideDistributed } from '$distributed/public';

	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const signedIn = $derived(!!data.session?.user);

	const initialData = untrack(() => data);
	const guestAtMount = untrack(() => !initialData.session?.user);
	const guestBootstrap =
		guestAtMount &&
		initialData.distributed !== undefined &&
		initialData.distributedAuthority !== undefined;

	const pageData = createPageDataSessionSource(initialData);
	let appliedHydration: SveltekitReplicaHydration | undefined = guestBootstrap
		? initialData.distributed
		: undefined;
	let hydrationTimer: ReturnType<typeof setTimeout> | undefined;

	const client = guestAtMount
		? provideDistributed({
				boundaries: DISTRIBUTED_BOUNDARY_OPERATIONS,
				session: pageData.session,
				browser,
				...(guestBootstrap
					? {
							hydration: initialData.distributed!,
							authority: initialData.distributedAuthority!
						}
					: {})
			})
		: useDistributedSvelteKitClient();
	const layoutRetention = browser
		? client.retainLocation(
				{
					id: 'layout:/chat',
					pathname: untrack(() => page.url.pathname),
					kind: 'layout'
				},
				{
					search: untrack(() => page.url.searchParams),
					session: initialData.session,
					props: initialData
				}
			)
		: null;

	$effect(() => {
		if (!guestAtMount || signedIn) return;
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
		hydrationTimer = setTimeout(() => {
			hydrationTimer = undefined;
			client.hydrate(data.distributed!, data.distributedAuthority!);
		}, 0);
	});

	onDestroy(() => {
		if (hydrationTimer !== undefined) clearTimeout(hydrationTimer);
		layoutRetention?.release();
		if (guestAtMount) client.destroy();
	});
</script>

{@render children()}
