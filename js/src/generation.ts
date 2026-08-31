export type DistributedGenerationEnvelope = Readonly<{
	version: 1;
	generationId: string;
	releaseId: string;
	applicationId?: string;
	clientId?: string;
	schemaId?: string;
	protocolId?: string;
	topologyId?: string;
	workerId?: string;
	memberId?: string;
	compatibilityId?: string;
}>;

/** Parse a generation envelope without trusting unbounded process metadata. */
export function parseDistributedGenerationEnvelope(
	value: unknown,
	path = 'generation'
): DistributedGenerationEnvelope {
	const record = object(value, path);
	if (record.version !== 1) throw new TypeError(`${path}.version must be 1`);
	const parsed: Record<string, string | number> = {
		version: 1,
		generationId: identity(record.generationId, `${path}.generationId`),
		releaseId: identity(record.releaseId, `${path}.releaseId`)
	};
	for (const key of [
		'applicationId',
		'clientId',
		'schemaId',
		'protocolId',
		'topologyId',
		'workerId',
		'memberId',
		'compatibilityId'
	] as const) {
		if (record[key] !== undefined) parsed[key] = identity(record[key], `${path}.${key}`);
	}
	return Object.freeze(parsed) as DistributedGenerationEnvelope;
}

function object(value: unknown, path: string): Record<string, unknown> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		throw new TypeError(`${path} must be an object`);
	}
	return value as Record<string, unknown>;
}

function identity(value: unknown, path: string): string {
	if (
		typeof value !== 'string' ||
		value.length === 0 ||
		value.length > 512 ||
		value !== value.trim() ||
		/[\u0000-\u001f\u007f]/.test(value)
	) {
		throw new TypeError(`${path} must be a bounded stable identity`);
	}
	return value;
}
