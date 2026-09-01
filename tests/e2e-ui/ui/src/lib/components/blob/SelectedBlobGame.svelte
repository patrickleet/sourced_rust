<script lang="ts">
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import { SelectedBlobGame } from '$distributed';

	const gameId = untrack(() => page.params.gameId);
	const query = SelectedBlobGame.use(gameId === undefined ? {} : { gameId });
	const selected = $derived($query.data.blob_games?.[0]);
</script>

{#if gameId}
	<span
		class="sr-only"
		data-testid="selected-blob-island"
		data-game-id={selected?.game_id ?? gameId}
		data-query-complete={$query.complete ? '1' : '0'}
	>
		Selected game {selected?.game_id ?? gameId}
	</span>
{/if}
