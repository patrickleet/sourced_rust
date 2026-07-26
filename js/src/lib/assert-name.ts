/** Require a non-empty string (field names, layer ids, etc.). */
export function assertName(
	value: unknown,
	description: string
): asserts value is string {
	if (typeof value !== 'string' || value.length === 0) {
		throw new TypeError(`${description} must be a non-empty string`);
	}
}
