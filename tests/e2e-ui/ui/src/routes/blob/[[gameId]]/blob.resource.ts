import { defineResource } from '$lib/gql/define-resource';
import { BlobGamesDocument, type BlobGamesQuery } from './blob.generated';

export type BlobGameRow = BlobGamesQuery['blob_games'][number];
export type BlobGamesQueryData = BlobGamesQuery;

export const blobGames = defineResource<BlobGamesQueryData>({
	query: BlobGamesDocument,
	select: (data) => data.blob_games
});
