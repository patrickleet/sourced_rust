import { isPlainRecord } from '../../../lib/is-plain-record.js';

export function defineBoundCommand(
	root: Record<string, unknown>,
	path: string,
	command: unknown
): void {
	const segments = commandPathSegments(path);
	let namespace = root;
	for (let index = 0; index < segments.length; index += 1) {
		const segment = segments[index]!;
		const leaf = index === segments.length - 1;
		const exists = Object.prototype.hasOwnProperty.call(namespace, segment);
		if (leaf) {
			if (exists) commandNamespaceCollision(path);
			Object.defineProperty(namespace, segment, {
				enumerable: true,
				configurable: false,
				writable: false,
				value: command
			});
			continue;
		}
		if (exists) {
			const value = namespace[segment];
			if (!isPlainRecord(value)) commandNamespaceCollision(path);
			namespace = value as Record<string, unknown>;
			continue;
		}
		const child = Object.create(null) as Record<string, unknown>;
		Object.defineProperty(namespace, segment, {
			enumerable: true,
			configurable: false,
			writable: false,
			value: child
		});
		namespace = child;
	}
}

export function commandPathSegments(path: string): readonly string[] {
	if (path.length === 0 || path.length > 512) {
		throw new TypeError('replica command path is invalid');
	}
	const segments = path.split('.');
	if (
		segments.length > 64 ||
		segments.some(
			(segment) =>
				segment.length === 0 ||
				segment.length > 128 ||
				segment.trim() !== segment ||
				/[\u0000-\u001f\u007f-\u009f]/.test(segment) ||
				segment === '__proto__' ||
				segment === 'prototype' ||
				segment === 'constructor'
		)
	) {
		throw new TypeError(`replica command path ${path} is invalid`);
	}
	return Object.freeze(segments);
}

export function commandNamespaceCollision(path: string): never {
	throw new TypeError(`replica command namespace collision at ${path}`);
}

export function freezeCommandTree(value: Record<string, unknown>): void {
	for (const child of Object.values(value)) {
		if (isPlainRecord(child)) {
			freezeCommandTree(child as Record<string, unknown>);
		}
	}
	Object.freeze(value);
}

