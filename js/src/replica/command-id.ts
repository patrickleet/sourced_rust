/** Create a browser/Node UUIDv7 command identity. */
export function createReplicaCommandId(): string {
	const crypto = globalThis.crypto;
	if (!crypto || typeof crypto.getRandomValues !== 'function') {
		throw new Error('replica commands require crypto.getRandomValues');
	}

	const bytes = crypto.getRandomValues(new Uint8Array(16));
	let timestamp = Date.now();
	for (let index = 5; index >= 0; index -= 1) {
		bytes[index] = timestamp & 0xff;
		timestamp = Math.floor(timestamp / 256);
	}
	bytes[6] = (bytes[6]! & 0x0f) | 0x70;
	bytes[8] = (bytes[8]! & 0x3f) | 0x80;

	const hex = [...bytes].map((value) => value.toString(16).padStart(2, '0'));
	return `${hex.slice(0, 4).join('')}-${hex.slice(4, 6).join('')}-${hex
		.slice(6, 8)
		.join('')}-${hex.slice(8, 10).join('')}-${hex.slice(10).join('')}`;
}
