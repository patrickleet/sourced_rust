/**
 * Peel accidental outer quotes from env values (Make-include / double-wrap pollution).
 * Shared by Auth.js config and server GraphQL base URL resolution.
 */
export function cleanEnvValue(raw: string | undefined | null): string {
	let s = (raw ?? '').trim();
	for (let i = 0; i < 2; i++) {
		if (
			s.length >= 2 &&
			((s.startsWith("'") && s.endsWith("'")) || (s.startsWith('"') && s.endsWith('"')))
		) {
			s = s.slice(1, -1).trim();
		} else {
			break;
		}
	}
	return s;
}
