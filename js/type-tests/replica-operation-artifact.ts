import type {
	ReplicaOperationArtifact,
	ReplicaObjectMember,
	ReplicaObjectSelection,
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
	version: 1,
	schemaHash: 'schema:type-test',
	surface: {
		kind: 'role',
		name: 'user'
	},
	operation: 'query:type-test',
	trustedPresets: []
} as const satisfies ReplicaOperationProtocol;

// @ts-expect-error Generated protocol artifacts always name their client surface.
export const protocolWithoutSurface: ReplicaOperationProtocol = {
	version: 1,
	schemaHash: 'schema:type-test',
	operation: 'query:type-test',
	trustedPresets: []
};

// @ts-expect-error Generated protocol artifacts always carry the exact preset union.
export const protocolWithoutTrustedPresets: ReplicaOperationProtocol = {
	version: 1,
	schemaHash: 'schema:type-test',
	surface: { kind: 'role', name: 'user' },
	operation: 'query:type-test'
};

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

// @ts-expect-error Artifacts without protocol-v1 binding are unsupported.
export const unboundArtifact: ReplicaOperationArtifact<Data, Variables> = {
	id: 'unbound:type-test',
	document: 'query TypeTest { items { id } }',
	roots: []
};

// @ts-expect-error Protocol-v1 artifacts must include their variable codec.
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

const handwrittenEntitySelection = {
	model: { id: 'Item', identityFields: ['id'] },
	fields: []
} as const;

// @ts-expect-error The generated recursive object IR is the only selection shape.
export const rejectedHandwrittenSelection: ReplicaObjectSelection =
	handwrittenEntitySelection;

// @ts-expect-error Generated scalars must bind an exact codec and nullability.
export const rejectedUnboundScalar: ReplicaObjectMember = {
	kind: 'scalar',
	responseKey: 'id',
	field: 'id'
};

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
