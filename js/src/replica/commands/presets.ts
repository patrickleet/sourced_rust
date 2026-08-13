import {
	isDistributedTrustedPresetCodec,
	parseDistributedTrustedPresetInventory,
	type DistributedTrustedPreset
} from '../../protocol.js';
import type { ReplicaValue } from '../types.js';
import { isPlainRecord } from '../../lib/is-plain-record.js';
import type {
	ReplicaMatchedTrustedPresetInventory,
	ReplicaTrustedPresetDescriptor
} from './types.js';
import {
	artifactInvalid,
	trustedPresetMismatch
} from './util.js';

export function matchReplicaTrustedPresetInventory(
	expected: readonly ReplicaTrustedPresetDescriptor[],
	authoritative: readonly DistributedTrustedPreset[]
): ReplicaMatchedTrustedPresetInventory {
	const descriptors = parseReplicaTrustedPresetDescriptors(expected);
	let values: readonly DistributedTrustedPreset[];
	try {
		values = parseDistributedTrustedPresetInventory(
			authoritative,
			'authoritativePresets'
		);
	} catch {
		trustedPresetMismatch('authoritativePresets');
	}
	if (descriptors.length !== values.length) {
		trustedPresetMismatch('authoritativePresets');
	}
	const byName = new Map(values.map((preset) => [preset.name, preset] as const));
	for (let index = 0; index < descriptors.length; index += 1) {
		const descriptor = descriptors[index]!;
		const value = byName.get(descriptor.name);
		if (value === undefined || value.codec !== descriptor.codec) {
			trustedPresetMismatch(`authoritativePresets.${descriptor.name}`);
		}
	}
	const resolve = (name: string): ReplicaValue => {
		const value = byName.get(name);
		if (value === undefined) {
			trustedPresetMismatch(`authoritativePresets.${name}`);
		}
		return value.value as ReplicaValue;
	};
	return Object.freeze({
		descriptors,
		values,
		resolve
	});
}

export function selectReplicaTrustedPresetInventory(
	expected: readonly ReplicaTrustedPresetDescriptor[],
	authoritative: readonly DistributedTrustedPreset[]
): ReplicaMatchedTrustedPresetInventory {
	let values: readonly DistributedTrustedPreset[];
	try {
		values = parseDistributedTrustedPresetInventory(
			authoritative,
			'authoritativePresets'
		);
	} catch {
		trustedPresetMismatch('authoritativePresets');
	}
	const names = new Set(expected.map(({ name }) => name));
	return matchReplicaTrustedPresetInventory(
		expected,
		values.filter(({ name }) => names.has(name))
	);
}

/**
 * Verify one parsed server receipt against the immutable prepared contract.
 *
 * This does not drive command status or overlay retirement. It only decides
 * whether task 9 may safely consume the receipt as matching evidence.
 */
export function parseReplicaTrustedPresetDescriptors(
	value: unknown,
	path = 'artifact.trustedPresets'
): readonly ReplicaTrustedPresetDescriptor[] {
	if (!Array.isArray(value)) artifactInvalid(path);
	const names = new Set<string>();
	return Object.freeze(
		value.map((candidate, index) => {
			const itemPath = `${path}[${index}]`;
			if (!isPlainRecord(candidate)) artifactInvalid(itemPath);
			const name = trustedPresetName(candidate.name, `${itemPath}.name`);
			if (names.has(name)) artifactInvalid(`${itemPath}.name`);
			names.add(name);
			if (!isDistributedTrustedPresetCodec(candidate.codec)) {
				artifactInvalid(`${itemPath}.codec`);
			}
			return Object.freeze({
				name,
				codec: candidate.codec
			});
		})
	);
}

export function trustedPresetName(value: unknown, path: string): string {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value.length > 128 ||
		value.trim() !== value ||
		/[\u0000-\u001f\u007f-\u009f]/.test(value)
	) {
		artifactInvalid(path);
	}
	return value;
}


