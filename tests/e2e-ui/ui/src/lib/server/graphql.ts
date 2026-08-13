/** Private API origin used by the package-owned SvelteKit SSR transport. */
import { env } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';
import { cleanEnvValue } from '$lib/clean-env';

export function apiBase(): string {
	return (
		cleanEnvValue(env.E2E_API_ORIGIN) ||
		cleanEnvValue(env.E2E_BASE_URL) ||
		cleanEnvValue(publicEnv.PUBLIC_E2E_API_ORIGIN) ||
		'http://127.0.0.1:8791'
	);
}

export function graphqlHttpUrl(): string {
	return `${apiBase()}/graphql`;
}
