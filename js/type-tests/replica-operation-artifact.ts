import type {
	ReplicaLegacyOperationArtifact,
	ReplicaOperationArtifact,
	ReplicaOperationProtocol,
	ReplicaPaginationArtifact,
	ReplicaProtocolOperationArtifact,
	ReplicaVariableCodecArtifact
} from '@hops-ops/distributed/replica';

type Data = {
	readonly items: readonly { readonly id: string }[];
};

type Variables = {
	readonly id: string;
};

const protocol = {
	version: 2,
	schemaHash: 'schema:type-test',
	operation: 'query:type-test'
} as const satisfies ReplicaOperationProtocol;

const variableCodec = {
	version: 2,
	limits: {
		maxDepth: 16,
		maxBoolWidth: 32,
		maxInList: 128
	},
	variables: {
		id: {
			kind: 'scalar',
			scalar: 'ID',
			codec: 'string',
			nullable: false
		}
	},
	inputs: {}
} as const satisfies ReplicaVariableCodecArtifact;

export const protocolArtifact: ReplicaProtocolOperationArtifact<Data, Variables> = {
	id: protocol.operation,
	document: 'query TypeTest($id: ID!) { items(id: $id) { id } }',
	roots: [],
	protocol,
	variableCodec
};

export const protocolArtifactViaUnion: ReplicaOperationArtifact<Data, Variables> =
	protocolArtifact;

export const legacyArtifact: ReplicaLegacyOperationArtifact<Data, Variables> = {
	id: 'legacy:type-test',
	document: 'query TypeTest { items { id } }',
	roots: []
};

export const legacyArtifactViaUnion: ReplicaOperationArtifact<Data, Variables> = {
	id: 'legacy:type-test-union',
	document: 'query TypeTest { items { id } }',
	roots: []
};

export const legacyArtifactWithCodec: ReplicaLegacyOperationArtifact<
	Data,
	Variables
> = {
	id: 'legacy:type-test-codec',
	document: 'query TypeTest($id: ID!) { items(id: $id) { id } }',
	roots: [],
	variableCodec
};

// @ts-expect-error Protocol-v2 artifacts must include their variable codec.
export const protocolWithoutCodec: ReplicaOperationArtifact<Data, Variables> = {
	id: protocol.operation,
	document: 'query TypeTest($id: ID!) { items(id: $id) { id } }',
	roots: [],
	protocol
};

type IndependentlyOptionalArtifact = Omit<
	ReplicaProtocolOperationArtifact<Data, Variables>,
	'protocol' | 'variableCodec'
> & {
	readonly protocol?: ReplicaOperationProtocol;
	readonly variableCodec?: ReplicaVariableCodecArtifact;
};

declare const independentlyOptionalArtifact: IndependentlyOptionalArtifact;

// @ts-expect-error Protocol and codec cannot be modeled as independent options.
export const rejectedIndependentOptions: ReplicaOperationArtifact<Data, Variables> =
	independentlyOptionalArtifact;

export const unprovenCursorPagination: ReplicaPaginationArtifact = {
	kind: 'cursor',
	insert: 'revalidate',
	delete: 'revalidate',
	reorder: 'revalidate',
	stableUpdate: 'revalidate'
};

export const forgedCertifiedCursor: ReplicaPaginationArtifact = {
	kind: 'cursor',
	// @ts-expect-error A boolean is not a versioned compiler cursor-proof IR.
	certified: true,
	insert: 'local',
	delete: 'local',
	reorder: 'local',
	stableUpdate: 'local'
};
