/** Build an immutable plain object from key/value pairs. */
export function freezeRecord<T>(
	entries: readonly (readonly [string, T])[]
): Readonly<Record<string, T>> {
	const result: Record<string, T> = {};
	for (const [key, value] of entries) {
		Object.defineProperty(result, key, {
			value,
			enumerable: true,
			configurable: false,
			writable: false
		});
	}
	return Object.freeze(result);
}
