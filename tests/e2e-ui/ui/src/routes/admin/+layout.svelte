<script lang="ts">
	import { browser } from '$app/environment';
	import { onDestroy, untrack } from 'svelte';
	import type { Snippet } from 'svelte';
	import {
		createPageDataSessionSource,
		type SveltekitReplicaHydration
	} from '@hops-ops/distributed/sveltekit';
	import { provideDistributed } from '$distributed/admin';

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
		hydrationTimer = setTimeout(() => {
			hydrationTimer = undefined;
			client.hydrate(data.distributed!, data.distributedAuthority!);
		}, 0);
	});

	onDestroy(() => {
		if (hydrationTimer !== undefined) clearTimeout(hydrationTimer);
		client.destroy();
	});
</script>

{@render children()}
