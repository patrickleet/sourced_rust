/** Lexicographic compare using UTF-16 code units (JS string order). */
export function compareCodeUnits(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}
