import {
	SHA256,
	ULID,
	ULID_ALPHABET,
	UUID_V7
} from './constants.js';
import type {
	ReplicaCommandContractErrorCode,
	ReplicaCommandScalarCodec
} from './types.js';
import { ReplicaCommandContractError } from './errors.js';
import { isDistributedTrustedPresetCodec } from '../../protocol.js';

export function createUlid(): string {
	const crypto = globalThis.crypto;
	if (!crypto || typeof crypto.getRandomValues !== 'function') {
		throw new ReplicaCommandContractError(
			'REPLICA_COMMAND_INPUT_INVALID',
			'inputDefaults.ulid'
		);
	}
	let timestamp = Date.now();
	let time = '';
	for (let index = 0; index < 10; index += 1) {
		time = ULID_ALPHABET[timestamp % 32]! + time;
		timestamp = Math.floor(timestamp / 32);
	}
	const bytes = crypto.getRandomValues(new Uint8Array(10));
	let random = 0n;
	for (const byte of bytes) random = (random << 8n) | BigInt(byte);
	let suffix = '';
	for (let index = 0; index < 16; index += 1) {
		suffix = ULID_ALPHABET[Number(random & 31n)]! + suffix;
		random >>= 5n;
	}
	return `${time}${suffix}`;
}

export function validateUuidV7(
	value: unknown,
	path: string,
	code: ReplicaCommandContractErrorCode
): string {
	if (typeof value !== 'string' || !UUID_V7.test(value)) {
		throw new ReplicaCommandContractError(code, path);
	}
	return value.toLowerCase();
}

export function validateUlid(value: unknown, path: string): string {
	if (typeof value !== 'string' || !ULID.test(value.toUpperCase())) {
		inputInvalid(path);
	}
	return value.toUpperCase();
}

export function sameProjectionMultiset(
	expected: readonly { projection: string; model: string }[],
	actual: readonly { projection: string; model: string }[]
): boolean {
	if (expected.length !== actual.length) return false;
	const counts = new Map<string, number>();
	for (const item of expected) {
		const key = JSON.stringify([item.projection, item.model]);
		counts.set(key, (counts.get(key) ?? 0) + 1);
	}
	for (const item of actual) {
		const key = JSON.stringify([item.projection, item.model]);
		const count = counts.get(key);
		if (count === undefined) return false;
		if (count === 1) counts.delete(key);
		else counts.set(key, count - 1);
	}
	return counts.size === 0;
}

export function samePath(left: readonly string[], right: readonly string[]): boolean {
	return (
		left.length === right.length &&
		left.every((segment, index) => segment === right[index])
	);
}

export function isSupportedCodec(value: string): value is ReplicaCommandScalarCodec {
	return isDistributedTrustedPresetCodec(value);
}

export function defineEnumerable<T>(
	target: Record<string, T>,
	key: string,
	value: T
): void {
	Object.defineProperty(target, key, {
		value,
		enumerable: true,
		configurable: true,
		writable: true
	});
}

export function nonempty(value: unknown, path: string): asserts value is string {
	if (typeof value !== 'string' || value.trim() === '') artifactInvalid(path);
}

export function requiredString(value: unknown, path: string): string {
	nonempty(value, path);
	return value;
}

export function hash(value: unknown, path: string): void {
	if (typeof value !== 'string' || !SHA256.test(value)) artifactInvalid(path);
}

export function artifactInvalid(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_ARTIFACT_INVALID',
		path
	);
}

export function inputInvalid(path: string): never {
	throw new ReplicaCommandContractError('REPLICA_COMMAND_INPUT_INVALID', path);
}

export function receiptMismatch(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_RECEIPT_MISMATCH',
		path
	);
}

export function trustedPresetMismatch(path: string): never {
	throw new ReplicaCommandContractError(
		'REPLICA_COMMAND_TRUSTED_PRESET_MISMATCH',
		path
	);
}

