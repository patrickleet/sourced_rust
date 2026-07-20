import { loadQuery } from '$lib/gql/load-query.server';
import { blobGames } from './blob.resource';
import type { BlobGamesQueryData } from './blob.resource';

/**
 * SSR seed — auth gate is hooks.server.ts (redirects unauthenticated to /login).
 * Same pattern as todos: loadQuery always returns session + games for the binder.
 */
export const load = loadQuery<BlobGamesQueryData, { games: BlobGamesQueryData['blob_games'] }>(
	blobGames.query,
	(data) => ({
		games: data?.blob_games ?? []
	})
);
