/** True for non-null objects that are not arrays (own enumerable bag). */
export function isPlainRecord(
	value: unknown
): value is Record<string, unknown> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		return false;
	}
	const proto = Object.getPrototypeOf(value);
	return proto === Object.prototype || proto === null;
}
