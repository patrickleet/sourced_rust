<script lang="ts">
	/**
	 * When signed out, install the e2e-ui-public client so ChatMessages open with
	 * the anonymous surface. Signed-in traffic keeps the root portable user client.
	 */
	import { browser } from '$app/environment';
	import { onDestroy, untrack } from 'svelte';
	import type { Snippet } from 'svelte';
	import {
		createPageDataSessionSource,
		type SveltekitReplicaHydration
	} from '@hops-ops/distributed/sveltekit';
	import { provideDistributed } from '$distributed/public';

	import type { LayoutData } from './$types';

	let { data, children }: { data: LayoutData; children: Snippet } = $props();

	const signedIn = $derived(!!data.session?.user);

	const initialData = untrack(() => data);
	const guestBootstrap = untrack(
		() =>
			!initialData.session?.user &&
			initialData.distributed !== undefined &&
			initialData.distributedAuthority !== undefined
	);

	const pageData = createPageDataSessionSource(initialData);
	let appliedHydration: SveltekitReplicaHydration | undefined = guestBootstrap
		? initialData.distributed
		: undefined;
	let hydrationTimer: ReturnType<typeof setTimeout> | undefined;

	const client = guestBootstrap
		? provideDistributed({
				session: pageData.session,
				browser,
				hydration: initialData.distributed!,
				authority: initialData.distributedAuthority!
			})
		: null;

	$effect(() => {
		if (!client || signedIn) return;
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
		client?.destroy();
	});
</script>

{@render children()}
