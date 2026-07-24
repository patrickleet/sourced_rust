import {
	isDistributedTrustedPresetCodec,
	parseGraphqlResponseExtensions,
	type DistributedCommandMetadata,
	type DistributedCommandState,
	type DistributedProtocolEnvelope,
	type DistributedTrustedPreset
} from '../protocol.js';
import type { GqlError } from '../types.js';
import {
	matchReplicaTrustedPresetInventory,
	prepareReplicaCommandWithTrustedPresets,
	verifyReplicaCommandReceipt,
	type PrepareReplicaCommandOptions,
	type ReplicaCommandArtifact,
	type ReplicaPreparedCommand,
	type ReplicaPreparedCommandEffect,
	type ReplicaPreparedEffectKey,
	type ReplicaTrustedPresetDescriptor
} from './commands.js';
import { replicaRecordKey } from './identity.js';
import type { ReplicaIndexSemanticChange } from './index-maintenance.js';
import type { ReplicaDiagnosticsSink } from './diagnostics.js';
import type {
	DistributedReplica,
	ReplicaAuthoritativeScope,
	ReplicaBaseWriter,
	ReplicaClientSurface,
	ReplicaModelArtifact,
	ReplicaOptimisticWriter,
	ReplicaResultEnvelope,
	ReplicaValue
} from './types.js';

const MAX_TRANSPORT_RETRIES = 8;
const MAX_OUTPUT_DEPTH = 64;
const INITIAL_STATUS_POLL_MS = 25;
const MAX_STATUS_POLL_MS = 1_000;
const SHA256 = /^sha256:[0-9a-f]{64}$/;

/**
 * Package-private authority handshake implemented by DistributedReplica.
 *
 * This symbol is intentionally not exported from the public replica barrel.
 * Generated command binders can therefore consume server-derived preset values,
 * while application code cannot pass an ambient caller-owned inventory.
 *
 * @internal
 */
export const replicaCommandAuthority = Symbol('distributed.replica.command-authority');
/**
 * Package-private post-commit observation channel implemented by
 * DistributedReplica. Generated command runtimes register here so framework
 * adapters never guess replica commit ordering.
 *
 * @internal
 */
export const replicaResultObservation = Symbol(
	'distributed.replica.result-observation'
);
/** @internal Package-private; intentionally absent from the public barrel. */
export const replicaCommandProjectedLifecycle = Symbol(
	'distributed.replica.command-projected-lifecycle'
);

export type ReplicaCommandSurfaceContract = Readonly<{
	protocolVersion: 2;
	schemaHash: string;
	protocolHash: string;
	surface: ReplicaClientSurface;
	trustedPresets: readonly ReplicaTrustedPresetDescriptor[];
}>;

export type ReplicaCommandAuthoritySnapshot = Readonly<{
	generation: number;
	scope: ReplicaAuthoritativeScope | undefined;
	trustedPresets: readonly DistributedTrustedPreset[];
	/** Aborted by the replica when this authorization generation closes. */
	signal?: AbortSignal;
}>;

export type ReplicaCommandAuthorityRegistration = Readonly<{
	read(): ReplicaCommandAuthoritySnapshot;
	dispose(): void;
}>;

export type ReplicaResultObservationRegistration = Readonly<{
	dispose(): void;
}>;

export type ReplicaCommandAuthorityHost = DistributedReplica & {
	readonly [replicaCommandAuthority]?: (
		contract: ReplicaCommandSurfaceContract
	) => ReplicaCommandAuthorityRegistration;
	readonly [replicaResultObservation]?: (
		observer: (envelope: ReplicaResultEnvelope<unknown>) => void
	) => ReplicaResultObservationRegistration;
};

export type ReplicaCommandTransportRequest = Readonly<{
	operation: 'mutation';
	commandName: string;
	commandId: string;
	mutationField: string;
	document: string;
	operationHash: string;
	variables: Readonly<Record<string, unknown>>;
	extensions: Readonly<Record<string, unknown>>;
	signal?: AbortSignal;
}>;

export type ReplicaCommandStatusArtifact = Readonly<{
	name: string;
	document: string;
	operationHash: string;
	protocol: Readonly<{
		version: 2;
		schemaHash: string;
		protocolHash: string;
		surface: ReplicaClientSurface;
		operation: string;
		/** Exact surface union: command presets plus client-evaluable policy claims. */
		trustedPresets: readonly ReplicaTrustedPresetDescriptor[];
	}>;
}>;

export type ReplicaCommandStatusRequest = Readonly<{
	operation: 'status';
	commandId: string;
	name: string;
	document: string;
	operationHash: string;
	variables: Readonly<{ commandId: string }>;
	extensions: Readonly<Record<string, unknown>>;
	signal?: AbortSignal;
}>;

export type ReplicaCommandTransportResult = Readonly<{
	data?: Readonly<Record<string, unknown>> | null;
	errors?: readonly GqlError[];
	extensions?: unknown;
	status: number;
}>;

export interface ReplicaCommandTransport {
	dispatch(
		request: ReplicaCommandTransportRequest
	): Promise<ReplicaCommandTransportResult>;
	/** Execute the exact compiler-owned command-status operation. */
	status?(
		request: ReplicaCommandStatusRequest
	): Promise<ReplicaCommandTransportResult>;
}

export type ReplicaCommandProjectedOutcome<TOutput> = Readonly<{
	commandId: string;
	state: 'projected';
	/** Present for same-transaction Projected<T>, absent for async facts. */
	result?: TOutput;
	metadata?: DistributedCommandMetadata;
}>;

export type ReplicaCommandStatus = Readonly<{
	commandId: string;
	state: DistributedCommandState;
	/**
	 * Durable causal evidence. The server deliberately omits it for the
	 * non-enumerating `unknown` and compact `expired` states.
	 */
	metadata?: DistributedCommandMetadata;
}>;

export type ReplicaCommandReceipt<TOutput> = Readonly<{
	commandId: string;
	state: Extract<
		DistributedCommandState,
		'accepted' | 'accepted_pending_projection' | 'projected'
	>;
	/** Typed application payload returned by the generated mutation. */
	result: TOutput;
	metadata: DistributedCommandMetadata;
	/** One exact generated status read. Calls coalesce while in flight. */
	status(): Promise<ReplicaCommandStatus>;
	/**
	 * Causal visibility awaitable. It is omitted when no finite projection
	 * contract exists and never resolves because a wall-clock timer elapsed.
	 */
	projected?: Promise<ReplicaCommandProjectedOutcome<TOutput>>;
}>;

type ReplicaCommandReceiptWithLifecycle = ReplicaCommandReceipt<unknown> &
	Readonly<{
		[replicaCommandProjectedLifecycle]?: Promise<
			ReplicaCommandProjectedOutcome<unknown>
		>;
	}>;

/**
 * Package-private causal lifecycle used by framework adapters.
 *
 * A caller-scoped `receipt.projected` can reject when its AbortSignal fires
 * after acceptance. The underlying command remains globally pending until
 * canonical projection evidence settles this independent promise.
 *
 * @internal
 */
export function replicaCommandProjectedLifecycleOf(
	receipt: ReplicaCommandReceipt<unknown>
): Promise<ReplicaCommandProjectedOutcome<unknown>> | undefined {
	return (receipt as ReplicaCommandReceiptWithLifecycle)[
		replicaCommandProjectedLifecycle
	];
}

/**
 * Stable recovery handle attached to an ambiguous dispatch error.
 *
 * `status()` never infers success from time. The original immutable prepared
 * mutation remains retained by the runtime/layer; status reports the durable
 * server outcome without manufacturing rollback or confirmation.
 */
export type ReplicaCommandRecoveryReceipt<TOutput = unknown> = Readonly<{
	commandId: string;
	status(): Promise<ReplicaCommandStatus>;
	/** Phantom slot retains the originating generated output type. */
	readonly __output?: TOutput;
}>;

export type ReplicaCommandCallOptions<TOutput> = PrepareReplicaCommandOptions &
	Readonly<{
		/**
		 * Cancels dispatch while the command outcome is unknown. After finite
		 * acceptance it instead bounds only this caller's `receipt.projected`
		 * wait; causal tracking and the optimistic layer continue independently.
		 */
		signal?: AbortSignal;
		/** Exact same prepared request retries after thrown/ambiguous transport. */
		transportRetries?: number;
		onAccepted?: (
			receipt: ReplicaCommandReceipt<TOutput>
		) => void | Promise<void>;
	}>;

export type ReplicaCommandRuntimeErrorCode =
	| 'REPLICA_COMMAND_ABORTED'
	| 'REPLICA_COMMAND_AUTHORITY_UNAVAILABLE'
	| 'REPLICA_COMMAND_DISPOSED'
	| 'REPLICA_COMMAND_OUTCOME_PENDING'
	| 'REPLICA_COMMAND_PROJECTION_FAILED'
	| 'REPLICA_COMMAND_PROTOCOL_INVALID'
	| 'REPLICA_COMMAND_REJECTED'
	| 'REPLICA_COMMAND_SCOPE_INVALIDATED'
	| 'REPLICA_COMMAND_STATUS_UNAVAILABLE'
	| 'REPLICA_COMMAND_TRANSPORT_AMBIGUOUS';

/** Redacted typed failure from the integrated generated-command lifecycle. */
export class ReplicaCommandRuntimeError extends Error {
	readonly code: ReplicaCommandRuntimeErrorCode;
	readonly commandId?: string;
	readonly state?: DistributedCommandState;
	readonly recovery?: ReplicaCommandRecoveryReceipt;

	constructor(
		code: ReplicaCommandRuntimeErrorCode,
		options: Readonly<{
			commandId?: string;
			state?: DistributedCommandState;
			cause?: unknown;
			recovery?: ReplicaCommandRecoveryReceipt;
		}> = {}
	) {
		super(commandRuntimeErrorMessage(code), {
			...(options.cause === undefined ? {} : { cause: options.cause })
		});
		this.name = 'ReplicaCommandRuntimeError';
		this.code = code;
		this.commandId = options.commandId;
		this.state = options.state;
		this.recovery = options.recovery;
	}
}

type AnyCommandArtifact = ReplicaCommandArtifact<unknown, unknown>;
type CommandEntry =
	| AnyCommandArtifact
	| Readonly<{ artifact: AnyCommandArtifact }>;

type ArtifactOf<TEntry> = TEntry extends ReplicaCommandArtifact<
	infer TInput,
	infer TOutput
>
	? ReplicaCommandArtifact<TInput, TOutput>
	: TEntry extends Readonly<{
				artifact: ReplicaCommandArtifact<infer TInput, infer TOutput>;
			}>
		? ReplicaCommandArtifact<TInput, TOutput>
		: never;

type InputOf<TEntry> =
	ArtifactOf<TEntry> extends ReplicaCommandArtifact<infer TInput, unknown>
		? TInput
		: never;

type OutputOf<TEntry> =
	ArtifactOf<TEntry> extends ReplicaCommandArtifact<unknown, infer TOutput>
		? TOutput
		: never;

export type ReplicaBoundCommand<TInput, TOutput> = [TInput] extends [void]
	? (
			options?: ReplicaCommandCallOptions<TOutput>
		) => Promise<ReplicaCommandReceipt<TOutput>>
	: (
			input: TInput,
			options?: ReplicaCommandCallOptions<TOutput>
		) => Promise<ReplicaCommandReceipt<TOutput>>;

type ReplicaBoundCommandPath<
	TPath extends string,
	TCommand
> = TPath extends `${infer THead}.${infer TTail}`
	? THead extends ''
		? never
		: Readonly<{
				[TKey in THead]: ReplicaBoundCommandPath<TTail, TCommand>;
			}>
	: TPath extends ''
		? never
		: Readonly<{ [TKey in TPath]: TCommand }>;

type UnionToIntersection<TValue> = (
	TValue extends unknown ? (value: TValue) => void : never
) extends (value: infer TIntersection) => void
	? TIntersection
	: never;

type SimplifyCommandTree<TValue> = TValue extends (
	...args: infer _TArguments
) => infer _TResult
	? TValue
	: Readonly<{
			[TKey in keyof TValue]: SimplifyCommandTree<TValue[TKey]>;
		}>;

export type ReplicaBoundCommands<
	TEntries extends Readonly<Record<string, CommandEntry>>
> = SimplifyCommandTree<
	UnionToIntersection<
		{
			[TKey in Extract<keyof TEntries, string>]: ReplicaBoundCommandPath<
				TKey,
				ReplicaBoundCommand<
					InputOf<TEntries[TKey]>,
					OutputOf<TEntries[TKey]>
				>
			>;
		}[Extract<keyof TEntries, string>]
	>
>;

export type ReplicaCommandRuntimeOptions = Readonly<{
	onBackgroundError?: (error: unknown) => void;
	/** Exact generated status operation for delayed/ambiguous recovery. */
	status?: ReplicaCommandStatusArtifact;
	/** Optional shared replica diagnostics sink used only for static artifact inspection. */
	diagnostics?: ReplicaDiagnosticsSink;
}>;

export interface ReplicaCommandRuntime<
	TEntries extends Readonly<Record<string, CommandEntry>>
> {
	readonly commands: ReplicaBoundCommands<TEntries>;
	/**
	 * Observe a query/live result only after the replica committed it.
	 *
	 * This completes pending causal awaitables from exact scoped observations;
	 * it does not normalize or confirm cache data itself.
	 */
	observeResult(envelope: ReplicaResultEnvelope<unknown>): void;
	dispose(): void;
}

type CapturedAuthority = Readonly<{
	generation: number;
	scope: ReplicaAuthoritativeScope;
	trustedPresets: readonly DistributedTrustedPreset[];
	signal?: AbortSignal;
}>;

type PendingProjection = {
	readonly commandId: string;
	readonly causationId: string;
	readonly authority: CapturedAuthority;
	readonly resolve: (
		value: ReplicaCommandProjectedOutcome<unknown>
	) => void;
	readonly reject: (error: unknown) => void;
	readonly promise: Promise<ReplicaCommandProjectedOutcome<unknown>>;
	readonly prepared: ReplicaPreparedCommand<unknown, unknown>;
	abort?: () => void;
	stopMonitor?: () => void;
	metadata: DistributedCommandMetadata;
	settled: boolean;
};

type CommandStatusTracker = {
	state: DistributedCommandState | undefined;
	metadata: DistributedCommandMetadata | undefined;
	inFlight: Promise<ReplicaCommandStatus> | undefined;
	pending?: PendingProjection;
};

type SemanticReplica = DistributedReplica & {
	createOptimisticLayer(
		id: string,
		update: (writer: ReplicaOptimisticWriter) => void,
		semanticChanges?: readonly ReplicaIndexSemanticChange[]
	): void;
};

/**
 * Bind generated descriptors into framework-neutral domain functions.
 *
 * The returned `commands.x(input)` functions are the only normal call-site API.
 * Cache effects, transport bytes, revalidation inventory, causal receipt
 * handling, and retry identity remain compiler/runtime owned.
 */
export function createReplicaCommandRuntime<
	const TEntries extends Readonly<Record<string, CommandEntry>>
>(
	replica: ReplicaCommandAuthorityHost,
	transport: ReplicaCommandTransport,
	entries: TEntries,
	options: ReplicaCommandRuntimeOptions = {}
): ReplicaCommandRuntime<TEntries> {
	const inventory = normalizeInventory(entries);
	if (options.diagnostics !== undefined) {
		for (const { artifact } of inventory) {
			try {
				options.diagnostics.command(artifact);
			} catch (error) {
				options.onBackgroundError?.(error);
			}
		}
	}
	const contract = commandSurfaceContract(
		inventory.map(({ artifact }) => artifact),
		options.status?.protocol.trustedPresets
	);
	const statusArtifact =
		options.status === undefined
			? undefined
			: commandStatusArtifact(options.status, contract);
	const replicaRevalidate =
		typeof replica.revalidate === 'function'
			? replica.revalidate.bind(replica)
			: undefined;
	if (statusArtifact !== undefined && transport.status === undefined) {
		throw new TypeError(
			'generated command status artifact requires transport.status'
		);
	}
	if (
		replicaRevalidate === undefined &&
		inventory.some(({ artifact }) => artifact.revalidation.required)
	) {
		throw new TypeError(
			'generated command revalidation plan requires replica.revalidate'
		);
	}
	const registration = replica[replicaCommandAuthority]?.(contract);
	const pending = new Map<string, PendingProjection>();
	const unmanagedLayers = new Set<string>();
	const runtimeAbort = new AbortController();
	let disposed = false;

	const rejectUnmanagedLayer = (commandId: string): boolean => {
		unmanagedLayers.delete(commandId);
		return replica.rejectOptimisticLayer(commandId);
	};

	const retireLayerAfterAuthoritativeRevalidation = (
		commandId: string
	): boolean => {
		if (!replica.markOptimisticLayerAccepted(commandId)) {
			// The authoritative result may already have retired this layer.
			unmanagedLayers.delete(commandId);
			return false;
		}
		replica.confirmOptimisticLayer(commandId, () => undefined);
		unmanagedLayers.delete(commandId);
		return true;
	};

	const authoritySnapshot = (): ReplicaCommandAuthoritySnapshot =>
		registration?.read() ?? {
			generation: replica.authorizationGeneration,
			scope: replica.scope,
			trustedPresets: Object.freeze([])
		};

	const currentAuthority = (): CapturedAuthority => {
		if (disposed) {
			throw new ReplicaCommandRuntimeError('REPLICA_COMMAND_DISPOSED');
		}
		const snapshot = authoritySnapshot();
		const scope = snapshot.scope;
		if (
			scope === undefined ||
			scope.protocolVersion !== contract.protocolVersion ||
			scope.schemaHash !== contract.schemaHash
		) {
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_AUTHORITY_UNAVAILABLE'
			);
		}
		const matched = matchReplicaTrustedPresetInventory(
			contract.trustedPresets,
			snapshot.trustedPresets
		);
		return Object.freeze({
			generation: snapshot.generation,
			scope: cloneScope(scope),
			trustedPresets: matched.values,
			...(snapshot.signal === undefined ? {} : { signal: snapshot.signal })
		});
	};

	const stillCurrent = (captured: CapturedAuthority): boolean => {
		const current = authoritySnapshot();
		return (
			!disposed &&
			current.generation === captured.generation &&
			current.scope !== undefined &&
			sameScope(current.scope, captured.scope)
		);
	};

	const revalidate = async (
		prepared: ReplicaPreparedCommand<unknown, unknown>,
		authoritySignal?: AbortSignal
	): Promise<boolean> => {
		if (replicaRevalidate === undefined) return false;
		const revalidationSignals = linkAbortSignals([
			authoritySignal,
			runtimeAbort.signal
		]);
		try {
			await waitForCommandOperation(
				replicaRevalidate(prepared.revalidation),
				revalidationSignals.signal
			);
			return true;
		} catch (error) {
			if (!disposed && authoritySignal?.aborted !== true) {
				options.onBackgroundError?.(error);
			}
			throw error;
		} finally {
			revalidationSignals.dispose();
		}
	};
	const revalidateInBackground = (
		prepared: ReplicaPreparedCommand<unknown, unknown>,
		authority?: CapturedAuthority
	): void => {
		void revalidate(prepared, authority?.signal).catch(() => undefined);
	};
	const revalidateAndConfirmUnmanagedInBackground = (
		prepared: ReplicaPreparedCommand<unknown, unknown>,
		authority: CapturedAuthority
	): void => {
		void revalidate(prepared, authority.signal)
			.then((refreshed) => {
				if (
					refreshed &&
					stillCurrent(authority) &&
					unmanagedLayers.has(prepared.commandId)
				) {
					/*
					 * A generated required revalidation is the canonical base
					 * fence for commands without a finite observation contract.
					 * Retire their accepted overlay only after that fetch settles.
					 */
					retireLayerAfterAuthoritativeRevalidation(
						prepared.commandId
					);
				}
			})
			.catch(() => undefined);
	};

	const statusReader = <TInput, TOutput>(
		prepared: ReplicaPreparedCommand<TInput, TOutput>,
		authority: CapturedAuthority,
		tracker: CommandStatusTracker
	): (() => Promise<ReplicaCommandStatus>) => {
		const read = async (): Promise<ReplicaCommandStatus> => {
			if (disposed) {
				throw new ReplicaCommandRuntimeError('REPLICA_COMMAND_DISPOSED', {
					commandId: prepared.commandId
				});
			}
			if (statusArtifact === undefined || transport.status === undefined) {
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_STATUS_UNAVAILABLE',
					{ commandId: prepared.commandId }
				);
			}
			if (!stillCurrent(authority)) {
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_SCOPE_INVALIDATED',
					{ commandId: prepared.commandId }
				);
			}
			let result: ReplicaCommandTransportResult;
			const statusSignals = linkAbortSignals([
				authority.signal,
				runtimeAbort.signal
			]);
			try {
				result = await waitForCommandOperation(
					transport.status(
						commandStatusRequest(
							statusArtifact,
							prepared.commandId,
							statusSignals.signal
						)
					),
					statusSignals.signal
				);
			} catch (error) {
				if (disposed) {
					throw new ReplicaCommandRuntimeError(
						'REPLICA_COMMAND_DISPOSED',
						{ commandId: prepared.commandId, cause: error }
					);
				}
				throw new ReplicaCommandRuntimeError(
					authority.signal?.aborted
						? 'REPLICA_COMMAND_SCOPE_INVALIDATED'
						: 'REPLICA_COMMAND_TRANSPORT_AMBIGUOUS',
					{ commandId: prepared.commandId, cause: error }
				);
			} finally {
				statusSignals.dispose();
			}
			if (!stillCurrent(authority)) {
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_SCOPE_INVALIDATED',
					{ commandId: prepared.commandId }
				);
			}
			let status: ReplicaCommandStatus;
			try {
				status = requireStatusEnvelope(
					result,
					statusArtifact,
					prepared,
					authority,
					contract
				);
				validateStatusProgression(tracker.state, tracker.metadata, status);
			} catch (error) {
				revalidateInBackground(prepared, authority);
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_PROTOCOL_INVALID',
					{ commandId: prepared.commandId, cause: error }
				);
			}
			tracker.state = status.state;
			if (status.metadata !== undefined) {
				tracker.metadata = status.metadata;
				if (tracker.pending !== undefined) {
					tracker.pending.metadata = status.metadata;
				}
			}
			switch (status.state) {
				case 'rejected':
					rejectUnmanagedLayer(prepared.commandId);
					failTrackedProjection(
						tracker,
						pending,
						new ReplicaCommandRuntimeError(
							'REPLICA_COMMAND_REJECTED',
							{
								commandId: prepared.commandId,
								state: status.state
							}
						)
					);
					break;
				case 'projection_failed':
				case 'expired':
					rejectUnmanagedLayer(prepared.commandId);
					revalidateInBackground(prepared, authority);
					failTrackedProjection(
						tracker,
						pending,
						new ReplicaCommandRuntimeError(
							'REPLICA_COMMAND_PROJECTION_FAILED',
							{
								commandId: prepared.commandId,
								state: status.state
							}
						)
					);
					break;
				case 'accepted':
				case 'accepted_pending_projection':
				case 'projected': {
					const metadata = status.metadata!;
					const remainsPending = replica.markOptimisticLayerAccepted(
						prepared.commandId,
						metadata
					);
					if (!remainsPending) {
						unmanagedLayers.delete(prepared.commandId);
						if (tracker.pending !== undefined) {
							settleTrackedProjection(tracker, pending);
						}
					} else if (
						metadata.state === 'projected' ||
						(metadata.state === 'accepted' &&
							prepared.confirmations === undefined &&
							prepared.revalidation.required)
					) {
						/*
						 * Status is causal truth, but it carries no canonical
						 * query payload. `projected` fences asynchronous
						 * projection; `accepted` is terminal when the generated
						 * contract has no confirmations. In both cases, only a
						 * query started after that fence may retire optimism.
						 */
						let refreshed = false;
						try {
							refreshed = await revalidate(
								prepared,
								authority.signal
							);
						} catch (error) {
							if (disposed) {
								throw new ReplicaCommandRuntimeError(
									'REPLICA_COMMAND_DISPOSED',
									{
										commandId: prepared.commandId,
										cause: error
									}
								);
							}
							if (
								authority.signal?.aborted ||
								!stillCurrent(authority)
							) {
								throw new ReplicaCommandRuntimeError(
									'REPLICA_COMMAND_SCOPE_INVALIDATED',
									{
										commandId: prepared.commandId,
										cause: error
									}
								);
							}
							// Keep the accepted layer and let the background
							// status monitor retry the exact generated plan.
						}
						if (disposed) {
							throw new ReplicaCommandRuntimeError(
								'REPLICA_COMMAND_DISPOSED',
								{ commandId: prepared.commandId }
							);
						}
						if (
							authority.signal?.aborted ||
							!stillCurrent(authority)
						) {
							throw new ReplicaCommandRuntimeError(
								'REPLICA_COMMAND_SCOPE_INVALIDATED',
								{ commandId: prepared.commandId }
							);
						}
						const controller = tracker.pending;
						if (refreshed) {
							if (
								controller !== undefined &&
								!controller.settled &&
								pending.get(prepared.commandId) === controller
							) {
								retireLayerAfterAuthoritativeRevalidation(
									prepared.commandId
								);
								settleTrackedProjection(tracker, pending);
							} else if (
								unmanagedLayers.has(prepared.commandId)
							) {
								/*
								 * Recovery receipts and commands without a
								 * finite projection awaitable have no
								 * PendingProjection controller. The same
								 * post-status canonical fetch still owns
								 * retirement of their retained overlay.
								 */
								retireLayerAfterAuthoritativeRevalidation(
									prepared.commandId
								);
							}
						}
					}
					break;
				}
				case 'unknown':
				case 'in_progress':
					break;
			}
			return status;
		};
		return () => {
			if (tracker.inFlight !== undefined) return tracker.inFlight;
			let current: Promise<ReplicaCommandStatus>;
			current = read().finally(() => {
				if (tracker.inFlight === current) tracker.inFlight = undefined;
			});
			tracker.inFlight = current;
			return current;
		};
	};

	const invoke = async <TInput, TOutput>(
		artifact: ReplicaCommandArtifact<TInput, TOutput>,
		input: TInput,
		callOptions: ReplicaCommandCallOptions<TOutput>
	): Promise<ReplicaCommandReceipt<TOutput>> => {
		const authority = currentAuthority();
		const prepared = prepareReplicaCommandWithTrustedPresets(
			artifact,
			input,
			authority.trustedPresets,
			callOptions
		);
		const transportRetries = normalizeRetries(callOptions.transportRetries);
		const semanticChanges = preparedSemanticChanges(prepared);
		const requestSignals = linkAbortSignals([
			authority.signal,
			callOptions.signal,
			runtimeAbort.signal
		]);
		const request = commandTransportRequest(prepared, requestSignals.signal);
		if (request.signal?.aborted) {
			requestSignals.dispose();
			throw new ReplicaCommandRuntimeError(
				stillCurrent(authority)
					? 'REPLICA_COMMAND_ABORTED'
					: 'REPLICA_COMMAND_SCOPE_INVALIDATED',
				{ commandId: prepared.commandId, cause: request.signal.reason }
			);
		}
		try {
			(replica as SemanticReplica).createOptimisticLayer(
				prepared.commandId,
				(writer) =>
					applyOptimisticEffects(writer, prepared.optimistic.operations),
				semanticChanges
			);
		} catch (error) {
			requestSignals.dispose();
			throw error;
		}
		unmanagedLayers.add(prepared.commandId);

		const statusTracker: CommandStatusTracker = {
			state: undefined,
			metadata: undefined,
			inFlight: undefined
		};
		const readStatus = statusReader(prepared, authority, statusTracker);
		const recovery: ReplicaCommandRecoveryReceipt<TOutput> = Object.freeze({
			commandId: prepared.commandId,
			status: readStatus
		});
		const recoveryForRetainedLayer = (
			revalidateOnRollback = false
		): ReplicaCommandRecoveryReceipt<TOutput> | undefined => {
			if (statusArtifact !== undefined) return recovery;
			/*
			 * A manual status-less runtime cannot make an ambiguous layer
			 * reachable again. Roll it back and let generated revalidation
			 * restore canonical truth instead of leaking optimism forever.
			 */
			rejectUnmanagedLayer(prepared.commandId);
			if (revalidateOnRollback) {
				revalidateInBackground(prepared, authority);
			}
			return undefined;
		};
		let result: ReplicaCommandTransportResult;
		let dispatchAttempted = false;
		try {
			result = await dispatchPrepared(
				transport,
				request,
				transportRetries,
				() => {
					dispatchAttempted = true;
				}
			);
		} catch (error) {
			if (disposed) {
				rejectUnmanagedLayer(prepared.commandId);
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_DISPOSED',
					{ commandId: prepared.commandId, cause: error }
				);
			}
			if (!dispatchAttempted) {
				rejectUnmanagedLayer(prepared.commandId);
				throw new ReplicaCommandRuntimeError(
					stillCurrent(authority)
						? 'REPLICA_COMMAND_ABORTED'
						: 'REPLICA_COMMAND_SCOPE_INVALIDATED',
					{ commandId: prepared.commandId, cause: error }
				);
			}
			if (!stillCurrent(authority)) {
				rejectUnmanagedLayer(prepared.commandId);
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_SCOPE_INVALIDATED',
					{ commandId: prepared.commandId, cause: error }
				);
			}
			throw new ReplicaCommandRuntimeError(
				request.signal?.aborted
					? 'REPLICA_COMMAND_ABORTED'
					: 'REPLICA_COMMAND_TRANSPORT_AMBIGUOUS',
				{
					commandId: prepared.commandId,
					cause: error,
					recovery: recoveryForRetainedLayer(true)
				}
			);
		} finally {
			requestSignals.dispose();
		}

		if (!stillCurrent(authority)) {
			rejectUnmanagedLayer(prepared.commandId);
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_SCOPE_INVALIDATED',
				{ commandId: prepared.commandId }
			);
		}

		let distributed: DistributedProtocolEnvelope;
		try {
			distributed = requireCommandEnvelope(result, prepared, authority);
			matchReplicaTrustedPresetInventory(
				contract.trustedPresets,
				distributed.trustedPresets
			);
		} catch (error) {
			revalidateInBackground(prepared, authority);
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROTOCOL_INVALID',
				{
					commandId: prepared.commandId,
					cause: error,
					recovery: recoveryForRetainedLayer()
				}
			);
		}

		const metadata = distributed.command!;
		statusTracker.state = metadata.state;
		statusTracker.metadata = metadata;
		if (metadata.state === 'rejected') {
			rejectUnmanagedLayer(prepared.commandId);
			throw new ReplicaCommandRuntimeError('REPLICA_COMMAND_REJECTED', {
				commandId: prepared.commandId,
				state: metadata.state
			});
		}
		if (metadata.state === 'expired') {
			rejectUnmanagedLayer(prepared.commandId);
			revalidateInBackground(prepared, authority);
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROJECTION_FAILED',
				{ commandId: prepared.commandId, state: metadata.state }
			);
		}
		if (metadata.state === 'unknown' || metadata.state === 'in_progress') {
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_OUTCOME_PENDING',
				{
					commandId: prepared.commandId,
					state: metadata.state,
					recovery: recoveryForRetainedLayer(true)
				}
			);
		}
		if (metadata.state === 'projection_failed') {
			rejectUnmanagedLayer(prepared.commandId);
			revalidateInBackground(prepared, authority);
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROJECTION_FAILED',
				{ commandId: prepared.commandId, state: metadata.state }
			);
		}
		if ((result.errors?.length ?? 0) > 0) {
			let retainedRecovery: ReplicaCommandRecoveryReceipt<TOutput> | undefined;
			if (prepared.confirmations?.kind === 'finite') {
				replica.markOptimisticLayerAccepted(prepared.commandId, metadata);
				retainedRecovery = recoveryForRetainedLayer();
			} else {
				rejectUnmanagedLayer(prepared.commandId);
			}
			revalidateInBackground(prepared, authority);
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROTOCOL_INVALID',
				{ commandId: prepared.commandId, recovery: retainedRecovery }
			);
		}

		let output: TOutput;
		try {
			output = commandOutput(
				artifact,
				result.data,
				prepared.transport.mutationField
			) as TOutput;
		} catch (error) {
			let retainedRecovery: ReplicaCommandRecoveryReceipt<TOutput> | undefined;
			if (
				prepared.consistency === 'projected' ||
				prepared.confirmations?.kind !== 'finite'
			) {
				rejectUnmanagedLayer(prepared.commandId);
			} else {
				replica.markOptimisticLayerAccepted(prepared.commandId, metadata);
				retainedRecovery = recoveryForRetainedLayer();
			}
			revalidateInBackground(prepared, authority);
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROTOCOL_INVALID',
				{
					commandId: prepared.commandId,
					cause: error,
					recovery: retainedRecovery
				}
			);
		}
		let projected:
			| Promise<ReplicaCommandProjectedOutcome<TOutput>>
			| undefined;
		let projectionLifecycle:
			| Promise<ReplicaCommandProjectedOutcome<TOutput>>
			| undefined;
		if (prepared.consistency === 'projected') {
			if (metadata.state !== 'projected') {
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_OUTCOME_PENDING',
					{
						commandId: prepared.commandId,
						state: metadata.state,
						recovery: recoveryForRetainedLayer(true)
					}
				);
			}
			try {
				confirmDirectProjection(replica, prepared, output, metadata);
				unmanagedLayers.delete(prepared.commandId);
			} catch (error) {
				rejectUnmanagedLayer(prepared.commandId);
				revalidateInBackground(prepared, authority);
				throw new ReplicaCommandRuntimeError(
					'REPLICA_COMMAND_PROTOCOL_INVALID',
					{ commandId: prepared.commandId, cause: error }
				);
			}
			projected = Promise.resolve(
				Object.freeze({
					commandId: prepared.commandId,
					state: 'projected' as const,
					result: output,
					metadata
				})
			);
		} else {
			const remainsPending = replica.markOptimisticLayerAccepted(
				prepared.commandId,
				metadata
			);
			if (prepared.confirmations?.kind === 'finite') {
				const controller = pendingProjection(
					authority,
					metadata,
					prepared as ReplicaPreparedCommand<unknown, unknown>
				);
				statusTracker.pending = controller;
				if (!remainsPending) {
					unmanagedLayers.delete(prepared.commandId);
					settleProjectionSuccess(controller);
				} else {
					/*
					 * Receipt/status observations prove durable causality, not
					 * that canonical query data was atomically ingested. Keep
					 * the layer and await DistributedReplica retirement even
					 * when every observation is already present here.
					 */
					pending.set(prepared.commandId, controller);
					unmanagedLayers.delete(prepared.commandId);
					attachAuthorityAbort(controller, () =>
						pending.delete(prepared.commandId)
					);
					if (
						statusArtifact !== undefined &&
						replicaRevalidate !== undefined
					) {
						monitorPendingProjection(
							controller,
							readStatus,
							() => pending.get(prepared.commandId) === controller,
							options.onBackgroundError
						);
					}
				}
				projectionLifecycle =
					controller.promise as Promise<
						ReplicaCommandProjectedOutcome<TOutput>
					>;
				projected = callerProjectedPromise(
					controller,
					callOptions.signal
				) as Promise<ReplicaCommandProjectedOutcome<TOutput>>;
			}
		}

		if (prepared.revalidation.required) {
			revalidateAndConfirmUnmanagedInBackground(prepared, authority);
		}

		const receiptValue = {
			commandId: prepared.commandId,
			state: metadata.state as ReplicaCommandReceipt<TOutput>['state'],
			result: output,
			metadata,
			status: readStatus,
			...(projected === undefined ? {} : { projected })
		};
		if (projectionLifecycle !== undefined) {
			Object.defineProperty(receiptValue, replicaCommandProjectedLifecycle, {
				value: projectionLifecycle,
				enumerable: false,
				configurable: false,
				writable: false
			});
		}
		const receipt = Object.freeze(
			receiptValue
		) as ReplicaCommandReceipt<TOutput>;
		try {
			await callOptions.onAccepted?.(receipt);
		} catch (error) {
			options.onBackgroundError?.(error);
		}
		return receipt;
	};

	const commands: Record<string, unknown> = Object.create(null) as Record<
		string,
		unknown
	>;
	for (const { key, artifact } of inventory) {
		defineBoundCommand(
			commands,
			key,
			artifact.input.kind === 'none'
				? (callOptions: ReplicaCommandCallOptions<unknown> = {}) =>
						invoke(artifact, undefined, callOptions)
				: (
						input: unknown,
						callOptions: ReplicaCommandCallOptions<unknown> = {}
					) => invoke(artifact, input, callOptions)
		);
	}
	freezeCommandTree(commands);

	const observeResult = (envelope: ReplicaResultEnvelope<unknown>): void => {
		if (disposed || pending.size === 0) return;
		let distributed: DistributedProtocolEnvelope | undefined;
		try {
			distributed = parseGraphqlResponseExtensions(envelope.extensions)
				?.distributed;
		} catch (error) {
			options.onBackgroundError?.(error);
			return;
		}
		if (distributed === undefined) return;
		for (const [commandId, controller] of pending) {
			if (!stillCurrent(controller.authority)) {
				settleProjectionFailure(
					controller,
					new ReplicaCommandRuntimeError(
						'REPLICA_COMMAND_SCOPE_INVALIDATED',
						{ commandId }
					)
				);
				pending.delete(commandId);
				continue;
			}
			if (
				distributed.schemaHash !== controller.authority.scope.schemaHash ||
				distributed.cacheScope !== controller.authority.scope.cacheScope
			) {
				continue;
			}
			const command = distributed.command;
			if (command?.commandId === commandId) {
				try {
					verifyReplicaCommandReceipt(controller.prepared, command);
				} catch (error) {
					revalidateInBackground(
						controller.prepared,
						controller.authority
					);
					settleProjectionFailure(
						controller,
						new ReplicaCommandRuntimeError(
							'REPLICA_COMMAND_PROTOCOL_INVALID',
							{ commandId, cause: error }
						)
					);
					pending.delete(commandId);
					continue;
				}
				if (command.causationId !== controller.causationId) {
					settleProjectionFailure(
						controller,
						new ReplicaCommandRuntimeError(
							'REPLICA_COMMAND_PROTOCOL_INVALID',
							{ commandId }
						)
					);
					pending.delete(commandId);
					continue;
				}
				controller.metadata = command;
				if (command.state === 'projection_failed') {
					replica.rejectOptimisticLayer(commandId);
					revalidateInBackground(
						controller.prepared,
						controller.authority
					);
					settleProjectionFailure(
						controller,
						new ReplicaCommandRuntimeError(
							'REPLICA_COMMAND_PROJECTION_FAILED',
							{ commandId, state: command.state }
						)
					);
					pending.delete(commandId);
					continue;
				}
				if (
					command.state === 'rejected' ||
					command.state === 'expired'
				) {
					replica.rejectOptimisticLayer(commandId);
					revalidateInBackground(
						controller.prepared,
						controller.authority
					);
					settleProjectionFailure(
						controller,
						new ReplicaCommandRuntimeError(
							command.state === 'rejected'
								? 'REPLICA_COMMAND_REJECTED'
								: 'REPLICA_COMMAND_PROJECTION_FAILED',
							{ commandId, state: command.state }
						)
					);
					pending.delete(commandId);
					continue;
				}
			}
			/*
			 * DistributedReplica is the authority on whether this frame's
			 * snapshot/observations were admissible. This callback runs only
			 * after that exact frame committed.
			 */
			const remainsPending =
				replica.markOptimisticLayerAccepted(commandId);
			if (!remainsPending) {
				settleProjectionSuccess(controller);
				pending.delete(commandId);
			}
		}
	};
	const observationRegistration =
		replica[replicaResultObservation]?.(observeResult);

	return Object.freeze({
		commands: commands as ReplicaBoundCommands<TEntries>,
		observeResult,
		dispose(): void {
			if (disposed) return;
			disposed = true;
			runtimeAbort.abort(
				new ReplicaCommandRuntimeError('REPLICA_COMMAND_DISPOSED')
			);
			observationRegistration?.dispose();
			registration?.dispose();
			for (const commandId of unmanagedLayers) {
				replica.rejectOptimisticLayer(commandId);
			}
			unmanagedLayers.clear();
			for (const [commandId, controller] of pending) {
				replica.rejectOptimisticLayer(commandId);
				settleProjectionFailure(
					controller,
					new ReplicaCommandRuntimeError('REPLICA_COMMAND_DISPOSED', {
						commandId
					})
				);
			}
			pending.clear();
		}
	});
}

function defineBoundCommand(
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

function commandPathSegments(path: string): readonly string[] {
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

function commandNamespaceCollision(path: string): never {
	throw new TypeError(`replica command namespace collision at ${path}`);
}

function freezeCommandTree(value: Record<string, unknown>): void {
	for (const child of Object.values(value)) {
		if (isPlainRecord(child)) {
			freezeCommandTree(child as Record<string, unknown>);
		}
	}
	Object.freeze(value);
}

function normalizeInventory<TEntries extends Readonly<Record<string, CommandEntry>>>(
	entries: TEntries
): readonly { readonly key: string; readonly artifact: AnyCommandArtifact }[] {
	if (entries === null || typeof entries !== 'object' || Array.isArray(entries)) {
		throw new TypeError('replica command inventory must be an object');
	}
	const names = new Set<string>();
	const inventory = Object.entries(entries).map(([key, entry]) => {
		commandPathSegments(key);
		const artifact = 'artifact' in entry ? entry.artifact : entry;
		if (names.has(artifact.name)) {
			throw new TypeError(`duplicate replica command artifact ${artifact.name}`);
		}
		names.add(artifact.name);
		return Object.freeze({ key, artifact });
	});
	const sortedPaths = inventory.map(({ key }) => key).sort(compareCodeUnits);
	for (let index = 1; index < sortedPaths.length; index += 1) {
		const previous = sortedPaths[index - 1]!;
		const current = sortedPaths[index]!;
		if (current.startsWith(`${previous}.`)) {
			commandNamespaceCollision(current);
		}
	}
	return Object.freeze(inventory);
}

function commandSurfaceContract(
	artifacts: readonly AnyCommandArtifact[],
	surfacePresets: readonly ReplicaTrustedPresetDescriptor[] | undefined
): ReplicaCommandSurfaceContract {
	if (artifacts.length === 0) {
		throw new TypeError('replica command inventory must not be empty');
	}
	const first = artifacts[0]!;
	const protocol = first.protocol;
	if (protocol.surface === undefined) {
		throw new TypeError('generated command protocol requires a client surface');
	}
	const trustedPresets = normalizePresetDescriptors(
		protocol.trustedPresets,
		'artifact.protocol.trustedPresets'
	);
	const commandPresets = new Map<string, ReplicaTrustedPresetDescriptor>();
	for (const artifact of artifacts) {
		if (
			artifact.protocol.version !== 2 ||
			artifact.protocol.schemaHash !== protocol.schemaHash ||
			artifact.protocol.protocolHash !== protocol.protocolHash ||
			!sameSurface(artifact.protocol.surface, protocol.surface) ||
			!samePresetDescriptors(
				normalizePresetDescriptors(
					artifact.protocol.trustedPresets,
					'artifact.protocol.trustedPresets'
				),
				trustedPresets
			)
		) {
			throw new TypeError(
				'replica command inventory spans incompatible client surfaces'
			);
		}
		for (const descriptor of artifact.trustedPresets ?? []) {
			const previous = commandPresets.get(descriptor.name);
			if (previous !== undefined && previous.codec !== descriptor.codec) {
				throw new TypeError(
					`trusted preset ${descriptor.name} has conflicting codecs`
				);
			}
			commandPresets.set(
				descriptor.name,
				Object.freeze({ name: descriptor.name, codec: descriptor.codec })
			);
		}
	}
	if (
		surfacePresets !== undefined &&
		!samePresetDescriptors(
			normalizePresetDescriptors(
				surfacePresets,
				'status.protocol.trustedPresets'
			),
			trustedPresets
		)
	) {
		throw new TypeError(
			'generated command status inventory does not match its client surface'
		);
	}
	const surfaceByName = new Map(
		trustedPresets.map((descriptor) => [descriptor.name, descriptor] as const)
	);
	for (const descriptor of commandPresets.values()) {
		if (surfaceByName.get(descriptor.name)?.codec !== descriptor.codec) {
			throw new TypeError(
				`command trusted preset ${descriptor.name} is absent from the client surface`
			);
		}
	}
	return Object.freeze({
		protocolVersion: 2,
		schemaHash: protocol.schemaHash,
		protocolHash: protocol.protocolHash,
		surface: cloneSurface(protocol.surface),
		trustedPresets
	});
}

function commandStatusArtifact(
	value: ReplicaCommandStatusArtifact,
	contract: ReplicaCommandSurfaceContract
): ReplicaCommandStatusArtifact {
	if (
		value === null ||
		typeof value !== 'object' ||
		typeof value.name !== 'string' ||
		value.name.trim().length === 0 ||
		typeof value.document !== 'string' ||
		value.document.trim().length === 0 ||
		typeof value.operationHash !== 'string' ||
		!SHA256.test(value.operationHash) ||
		value.protocol === null ||
		typeof value.protocol !== 'object' ||
		value.protocol.version !== 2 ||
		value.protocol.operation !== value.operationHash ||
		value.protocol.schemaHash !== contract.schemaHash ||
		value.protocol.protocolHash !== contract.protocolHash ||
		!sameSurface(value.protocol.surface, contract.surface)
	) {
		throw new TypeError('generated command status artifact is invalid');
	}
	const trustedPresets = normalizePresetDescriptors(
		value.protocol.trustedPresets,
		'status.protocol.trustedPresets'
	);
	if (!samePresetDescriptors(trustedPresets, contract.trustedPresets)) {
		throw new TypeError(
			'generated command status inventory does not match its client surface'
		);
	}
	return Object.freeze({
		name: value.name,
		document: value.document,
		operationHash: value.operationHash,
		protocol: Object.freeze({
			version: 2,
			schemaHash: contract.schemaHash,
			protocolHash: contract.protocolHash,
			surface: cloneSurface(contract.surface),
			operation: value.operationHash,
			trustedPresets
		})
	});
}

function normalizePresetDescriptors(
	value: readonly ReplicaTrustedPresetDescriptor[],
	path: string
): readonly ReplicaTrustedPresetDescriptor[] {
	if (!Array.isArray(value)) {
		throw new TypeError(`${path} must be an array`);
	}
	const names = new Set<string>();
	const result = value.map((descriptor, index) => {
		if (
			descriptor === null ||
			typeof descriptor !== 'object' ||
			typeof descriptor.name !== 'string' ||
			descriptor.name.length === 0 ||
			descriptor.name.length > 128 ||
			descriptor.name.trim() !== descriptor.name ||
			/[\u0000-\u001f\u007f-\u009f]/.test(descriptor.name) ||
			names.has(descriptor.name) ||
			!isDistributedTrustedPresetCodec(descriptor.codec)
		) {
			throw new TypeError(`${path}[${index}] is invalid`);
		}
		names.add(descriptor.name);
		return Object.freeze({
			name: descriptor.name,
			codec: descriptor.codec
		});
	});
	return Object.freeze(
		result.sort(({ name: left }, { name: right }) =>
			compareCodeUnits(left, right)
		)
	);
}

function samePresetDescriptors(
	left: readonly ReplicaTrustedPresetDescriptor[],
	right: readonly ReplicaTrustedPresetDescriptor[]
): boolean {
	return (
		left.length === right.length &&
		left.every(
			(descriptor, index) =>
				descriptor.name === right[index]?.name &&
				descriptor.codec === right[index]?.codec
		)
	);
}

function applyOptimisticEffects(
	writer: ReplicaOptimisticWriter,
	effects: readonly ReplicaPreparedCommandEffect[]
): void {
	for (const effect of effects) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch': {
				const model = modelFromKey(effect.model, effect.key);
				writer.writeRecord(model, identityFromKey(effect.key), {
					fields: fieldsFromEffect(effect.key, effect.fields)
				});
				break;
			}
			case 'delete':
				writer.tombstoneRecord(
					modelFromKey(effect.model, effect.key),
					identityFromKey(effect.key)
				);
				break;
			case 'link':
			case 'unlink':
			case 'invalidate_model':
			case 'invalidate_relationship':
				// Task 8 consumes the exact semantic context. Guessing a to-one
				// record link for a to-many relationship would corrupt truth.
				break;
		}
	}
}

function preparedSemanticChanges<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>
): readonly ReplicaIndexSemanticChange[] {
	const dependencies = Object.freeze([...prepared.revalidation.dependencies]);
	const changes: ReplicaIndexSemanticChange[] = [];
	for (const effect of prepared.optimistic.operations) {
		switch (effect.kind) {
			case 'upsert':
			case 'patch':
			case 'delete':
				// DistributedReplica captures ordinary writer mutations into the
				// same layer context. Supplying them again would double-apply the
				// semantic record operation.
				break;
			case 'link':
			case 'unlink': {
				const source = modelFromKey(
					effect.relationship.sourceModel,
					effect.source
				);
				const target = modelFromKey(
					effect.relationship.targetModel,
					effect.target
				);
				changes.push(
					Object.freeze({
						kind: effect.kind,
						sourceModel: effect.relationship.sourceModel,
						field: effect.relationship.field,
						targetModel: effect.relationship.targetModel,
						sourceKey: replicaRecordKey(
							source,
							identityFromKey(effect.source)
						),
						targetKey: replicaRecordKey(
							target,
							identityFromKey(effect.target)
						),
						dependencies
					})
				);
				break;
			}
			case 'invalidate_model':
			case 'invalidate_relationship':
				changes.push(
					Object.freeze({
						kind: 'invalidate',
						dependencies
					})
				);
				break;
		}
	}
	return Object.freeze(changes);
}

function modelFromKey(
	model: string,
	key: ReplicaPreparedEffectKey
): ReplicaModelArtifact {
	return Object.freeze({
		id: model,
		identityFields: Object.freeze(key.fields.map(({ field }) => field))
	});
}

function identityFromKey(
	key: ReplicaPreparedEffectKey
): readonly ReplicaValue[] {
	return Object.freeze(key.fields.map(({ value }) => value));
}

function fieldsFromEffect(
	key: ReplicaPreparedEffectKey,
	fields: readonly { readonly field: string; readonly value: ReplicaValue }[]
): Readonly<Record<string, ReplicaValue>> {
	const result: Record<string, ReplicaValue> = Object.create(null) as Record<
		string,
		ReplicaValue
	>;
	for (const field of [...key.fields, ...fields]) {
		Object.defineProperty(result, field.field, {
			enumerable: true,
			configurable: false,
			writable: false,
			value: field.value
		});
	}
	return Object.freeze(result);
}

function commandTransportRequest<TInput, TOutput>(
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	signal: AbortSignal | undefined
): ReplicaCommandTransportRequest {
	const surface = prepared.transport.protocol.surface;
	if (surface === undefined) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID',
			{ commandId: prepared.commandId }
		);
	}
	return Object.freeze({
		operation: 'mutation',
		commandName: prepared.name,
		commandId: prepared.commandId,
		mutationField: prepared.transport.mutationField,
		document: prepared.transport.document,
		operationHash: prepared.transport.operationHash,
		variables: prepared.transport.variables as Readonly<Record<string, unknown>>,
		extensions: Object.freeze({
			distributed: Object.freeze({
				client: Object.freeze({
					surface: cloneSurface(surface),
					schemaHash: prepared.transport.protocol.schemaHash
				})
			})
		}),
		...(signal === undefined ? {} : { signal })
	});
}

function commandStatusRequest(
	artifact: ReplicaCommandStatusArtifact,
	commandId: string,
	signal: AbortSignal | undefined
): ReplicaCommandStatusRequest {
	return Object.freeze({
		operation: 'status',
		commandId,
		name: artifact.name,
		document: artifact.document,
		operationHash: artifact.operationHash,
		variables: Object.freeze({ commandId }),
		extensions: Object.freeze({
			distributed: Object.freeze({
				client: Object.freeze({
					surface: cloneSurface(artifact.protocol.surface),
					schemaHash: artifact.protocol.schemaHash
				})
			})
		}),
		...(signal === undefined ? {} : { signal })
	});
}

async function dispatchPrepared(
	transport: ReplicaCommandTransport,
	request: ReplicaCommandTransportRequest,
	retries: number,
	onAttempt: () => void
): Promise<ReplicaCommandTransportResult> {
	let error: unknown;
	for (let attempt = 0; attempt <= retries; attempt += 1) {
		if (request.signal?.aborted) {
			throw request.signal.reason ?? new Error('command request aborted');
		}
		try {
			onAttempt();
			return await waitForCommandOperation(
				transport.dispatch(request),
				request.signal
			);
		} catch (candidate) {
			error = candidate;
			if (request.signal?.aborted) throw candidate;
		}
	}
	throw error;
}

function requireCommandEnvelope<TInput, TOutput>(
	result: ReplicaCommandTransportResult,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	authority: CapturedAuthority
): DistributedProtocolEnvelope {
	const distributed = parseGraphqlResponseExtensions(result.extensions)?.distributed;
	if (
		distributed === undefined ||
		distributed.command === undefined ||
		distributed.operation !== prepared.transport.operationHash ||
		distributed.protocolVersion !== authority.scope.protocolVersion ||
		distributed.schemaHash !== authority.scope.schemaHash ||
		distributed.cacheScope !== authority.scope.cacheScope
	) {
		throw new Error('command response does not match its generated scope');
	}
	verifyReplicaCommandReceipt(prepared, distributed.command);
	return distributed;
}

function requireStatusEnvelope<TInput, TOutput>(
	result: ReplicaCommandTransportResult,
	artifact: ReplicaCommandStatusArtifact,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	authority: CapturedAuthority,
	contract: ReplicaCommandSurfaceContract
): ReplicaCommandStatus {
	if (
		!Number.isSafeInteger(result.status) ||
		result.status < 200 ||
		result.status >= 300 ||
		(result.errors?.length ?? 0) !== 0
	) {
		throw new Error('command status request did not succeed');
	}
	const state = commandStatusOutput(result.data);
	const distributed = parseGraphqlResponseExtensions(result.extensions)?.distributed;
	if (
		distributed === undefined ||
		distributed.operation !== artifact.operationHash ||
		distributed.protocolVersion !== authority.scope.protocolVersion ||
		distributed.schemaHash !== authority.scope.schemaHash ||
		distributed.cacheScope !== authority.scope.cacheScope ||
		distributed.snapshot !== undefined ||
		distributed.live !== undefined
	) {
		throw new Error('command status response does not match its generated scope');
	}
	matchReplicaTrustedPresetInventory(
		contract.trustedPresets,
		distributed.trustedPresets
	);
	const metadata = distributed.command;
	if (metadata === undefined) {
		if (state !== 'unknown' && state !== 'expired') {
			throw new Error('command status omitted required causal metadata');
		}
		return Object.freeze({
			commandId: prepared.commandId,
			state
		});
	}
	verifyReplicaCommandReceipt(prepared, metadata);
	if (metadata.state !== state) {
		throw new Error('command status data and causal metadata disagree');
	}
	return Object.freeze({
		commandId: prepared.commandId,
		state,
		metadata
	});
}

function commandStatusOutput(
	data: Readonly<Record<string, unknown>> | null | undefined
): DistributedCommandState {
	if (
		data === undefined ||
		data === null ||
		!isPlainRecord(data) ||
		Reflect.ownKeys(data).length !== 1 ||
		!Object.prototype.hasOwnProperty.call(data, 'commandStatus')
	) {
		throw new Error('command status data has an invalid root shape');
	}
	const value = data.commandStatus;
	if (
		!isPlainRecord(value) ||
		Reflect.ownKeys(value).length !== 1 ||
		!Object.prototype.hasOwnProperty.call(value, 'state')
	) {
		throw new Error('command status data has an invalid result shape');
	}
	switch (value.state) {
		case 'in_progress':
		case 'accepted':
		case 'accepted_pending_projection':
		case 'projected':
		case 'rejected':
		case 'projection_failed':
		case 'expired':
		case 'unknown':
			return value.state;
		default:
			throw new Error('command status data has an invalid state');
	}
}

function validateStatusProgression(
	previousState: DistributedCommandState | undefined,
	previous: DistributedCommandMetadata | undefined,
	current: ReplicaCommandStatus
): void {
	if (
		previousState !== undefined &&
		!isStatusTransition(previousState, current.state)
	) {
		throw new Error('command status regressed or changed terminal outcome');
	}
	const next = current.metadata;
	if (next === undefined) {
		if (
			(current.state !== 'unknown' && current.state !== 'expired') ||
			(previous !== undefined && current.state === 'unknown')
		) {
			throw new Error('command status lost causal metadata');
		}
		return;
	}
	if (next.state !== current.state) {
		throw new Error('command status metadata has an inconsistent state');
	}
	if (previous === undefined) return;
	if (
		next.commandId !== previous.commandId ||
		next.causationId !== previous.causationId ||
		next.consistency !== previous.consistency
	) {
		throw new Error('command status changed causal identity');
	}
	if (
		!(
			previous.state === 'in_progress' &&
			previous.expects.length === 0
		) &&
		!sameStringMultiset(
			previous.expects.map(projectionExpectationFingerprint),
			next.expects.map(projectionExpectationFingerprint)
		)
	) {
		throw new Error('command status changed projection expectations');
	}
	if (
		!isStringSubset(
			previous.observations.map(projectionObservationFingerprint),
			next.observations.map(projectionObservationFingerprint)
		) ||
		!isStringSubset(
			previous.records.map(recordRevisionFingerprint),
			next.records.map(recordRevisionFingerprint)
		)
	) {
		throw new Error('command status lost causal evidence');
	}
}

function isStatusTransition(
	previous: DistributedCommandState,
	next: DistributedCommandState
): boolean {
	switch (previous) {
		case 'unknown':
			return true;
		case 'in_progress':
			return next !== 'unknown';
		case 'accepted':
			return next === 'accepted' || next === 'expired';
		case 'accepted_pending_projection':
			return (
				next === 'accepted_pending_projection' ||
				next === 'projected' ||
				next === 'projection_failed' ||
				next === 'expired'
			);
		case 'projected':
			return next === 'projected' || next === 'expired';
		case 'rejected':
			return next === 'rejected' || next === 'expired';
		case 'projection_failed':
			return next === 'projection_failed' || next === 'expired';
		case 'expired':
			return next === 'expired';
	}
}

function projectionExpectationFingerprint(
	value: DistributedCommandMetadata['expects'][number]
): string {
	return tupleFingerprint([
		value.projection,
		value.model,
		value.scopeToken
	]);
}

function projectionObservationFingerprint(
	value: DistributedCommandMetadata['observations'][number]
): string {
	return tupleFingerprint([
		value.causationId,
		value.projection,
		value.model,
		value.scopeToken
	]);
}

function recordRevisionFingerprint(
	value: DistributedCommandMetadata['records'][number]
): string {
	return tupleFingerprint([
		value.model,
		value.scopeToken,
		value.incarnation,
		value.revision,
		value.tombstone ? '1' : '0',
		...(value.path ?? [])
	]);
}

function tupleFingerprint(parts: readonly string[]): string {
	return parts.map((part) => `${part.length}:${part}`).join('');
}

function sameStringMultiset(
	left: readonly string[],
	right: readonly string[]
): boolean {
	if (left.length !== right.length) return false;
	const sortedLeft = [...left].sort(compareCodeUnits);
	const sortedRight = [...right].sort(compareCodeUnits);
	return sortedLeft.every((value, index) => value === sortedRight[index]);
}

function isStringSubset(
	subset: readonly string[],
	superset: readonly string[]
): boolean {
	const remaining = new Map<string, number>();
	for (const value of superset) {
		remaining.set(value, (remaining.get(value) ?? 0) + 1);
	}
	for (const value of subset) {
		const count = remaining.get(value) ?? 0;
		if (count === 0) return false;
		remaining.set(value, count - 1);
	}
	return true;
}

function commandOutput<TInput, TOutput>(
	artifact: ReplicaCommandArtifact<TInput, TOutput>,
	data: Readonly<Record<string, unknown>> | null | undefined,
	field: string
): unknown {
	if (
		data === undefined ||
		data === null ||
		!Object.prototype.hasOwnProperty.call(data, field) ||
		Reflect.ownKeys(data).some((key) => key !== field)
	) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID'
		);
	}
	return cloneOutputShape(artifact.output, data[field], `data.${field}`, 0);
}

function cloneOutputShape(
	shape: ReplicaCommandArtifact<unknown, unknown>['output'],
	value: unknown,
	path: string,
	depth: number
): unknown {
	if (depth > MAX_OUTPUT_DEPTH) outputInvalid(path);
	if (shape.kind !== 'object') outputInvalid(path);
	if (!isPlainRecord(value)) outputInvalid(path);
	const known = new Set(shape.definition.fields.map(({ name }) => name));
	for (const key of Reflect.ownKeys(value)) {
		if (typeof key !== 'string' || !known.has(key)) outputInvalid(`${path}.${String(key)}`);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (descriptor === undefined || !('value' in descriptor)) {
			outputInvalid(`${path}.${key}`);
		}
	}
	const output: Record<string, unknown> = {};
	for (const field of shape.definition.fields) {
		const present =
			Object.prototype.hasOwnProperty.call(value, field.name) &&
			value[field.name] !== undefined;
		if (!present) {
			outputInvalid(`${path}.${field.name}`);
		}
		const fieldValue = value[field.name];
		if (fieldValue === null) {
			if (!field.nullable) outputInvalid(`${path}.${field.name}`);
			output[field.name] = null;
			continue;
		}
		const cloneItem = (item: unknown, itemPath: string): unknown =>
			field.nested === undefined
				? cloneOutputScalar(field.codec, item, itemPath)
				: cloneOutputShape(
						{ kind: 'object', definition: field.nested },
						item,
						itemPath,
						depth + 1
					);
		if (field.list) {
			if (!Array.isArray(fieldValue)) outputInvalid(`${path}.${field.name}`);
			output[field.name] = Object.freeze(
				fieldValue.map((item, index) => {
					if (item === null) {
						if (!field.itemNullable) {
							outputInvalid(`${path}.${field.name}[${index}]`);
						}
						return null;
					}
					return cloneItem(item, `${path}.${field.name}[${index}]`);
				})
			);
		} else {
			output[field.name] = cloneItem(fieldValue, `${path}.${field.name}`);
		}
	}
	return Object.freeze(output);
}

function cloneOutputScalar(
	codec: string | undefined,
	value: unknown,
	path: string
): ReplicaValue {
	switch (codec) {
		case 'string':
		case 'string_unvalidated_timestamp':
		case 'base64':
			if (typeof value !== 'string') outputInvalid(path);
			return value;
		case 'boolean':
			if (typeof value !== 'boolean') outputInvalid(path);
			return value;
		case 'int32':
			if (
				typeof value !== 'number' ||
				!Number.isInteger(value) ||
				value < -2_147_483_648 ||
				value > 2_147_483_647
			) {
				outputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'json_number_precision_limited':
			if (typeof value !== 'number' || !Number.isInteger(value)) {
				outputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'float64':
			if (typeof value !== 'number' || !Number.isFinite(value)) {
				outputInvalid(path);
			}
			return Object.is(value, -0) ? 0 : value;
		case 'json':
			return cloneOutputJson(value, path, new Set(), 0);
		default:
			outputInvalid(`${path}.codec`);
	}
}

function cloneOutputJson(
	value: unknown,
	path: string,
	active: Set<object>,
	depth: number
): ReplicaValue {
	if (depth > MAX_OUTPUT_DEPTH) outputInvalid(path);
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean'
	) {
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) outputInvalid(path);
		return Object.is(value, -0) ? 0 : value;
	}
	if (typeof value !== 'object' || active.has(value)) outputInvalid(path);
	active.add(value);
	if (Array.isArray(value)) {
		const output = Object.freeze(
			value.map((item, index) =>
				cloneOutputJson(item, `${path}[${index}]`, active, depth + 1)
			)
		);
		active.delete(value);
		return output;
	}
	if (!isPlainRecord(value)) outputInvalid(path);
	const output: Record<string, ReplicaValue> = {};
	for (const key of Reflect.ownKeys(value).sort(comparePropertyKeys)) {
		if (typeof key !== 'string') outputInvalid(path);
		const descriptor = Object.getOwnPropertyDescriptor(value, key);
		if (
			descriptor === undefined ||
			!('value' in descriptor) ||
			descriptor.value === undefined
		) {
			outputInvalid(`${path}.${key}`);
		}
		output[key] = cloneOutputJson(
			descriptor.value,
			`${path}.${key}`,
			active,
			depth + 1
		);
	}
	active.delete(value);
	return Object.freeze(output);
}

function confirmDirectProjection<TInput, TOutput>(
	replica: DistributedReplica,
	prepared: ReplicaPreparedCommand<TInput, TOutput>,
	output: TOutput,
	metadata: DistributedCommandMetadata
): void {
	const direct = prepared.directProjection;
	if (direct === undefined || !isPlainRecord(output)) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID',
			{ commandId: prepared.commandId }
		);
	}
	const identity: ReplicaValue[] = [];
	for (const field of direct.identityFields) {
		const value = output[field];
		if (value === undefined || value === null) {
			throw new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_PROTOCOL_INVALID',
				{ commandId: prepared.commandId }
			);
		}
		identity.push(value as ReplicaValue);
	}
	const evidence = metadata.records.filter(
		(record) =>
			record.model === direct.model &&
			!record.tombstone &&
			(
				record.path === undefined ||
				(record.path.length === 1 &&
					record.path[0] === prepared.transport.mutationField)
			)
	);
	if (evidence.length !== 1) {
		throw new ReplicaCommandRuntimeError(
			'REPLICA_COMMAND_PROTOCOL_INVALID',
			{ commandId: prepared.commandId }
		);
	}
	const record = evidence[0]!;
	const model: ReplicaModelArtifact = Object.freeze({
		id: direct.model,
		identityFields: direct.identityFields
	});
	const fields = cloneOutputJson(
		output,
		'projected.output',
		new Set(),
		0
	) as Readonly<Record<string, ReplicaValue>>;
	replica.confirmOptimisticLayer(prepared.commandId, (writer: ReplicaBaseWriter) =>
		writer.writeRecord(model, Object.freeze(identity), record.revision, {
			incarnation: record.incarnation,
			fields
		})
	);
}

function pendingProjection(
	authority: CapturedAuthority,
	metadata: DistributedCommandMetadata,
	prepared: ReplicaPreparedCommand<unknown, unknown>
): PendingProjection {
	let resolve!: (value: ReplicaCommandProjectedOutcome<unknown>) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<ReplicaCommandProjectedOutcome<unknown>>(
		(resolvePromise, rejectPromise) => {
			resolve = resolvePromise;
			reject = rejectPromise;
		}
	);
	/*
	 * Authority loss or a terminal status can arrive before application code
	 * receives the accepted receipt. Mark the internal lifecycle promise handled
	 * eagerly while preserving its rejection for every explicit awaiter.
	 */
	void promise.catch(() => undefined);
	const controller: PendingProjection = {
		commandId: metadata.commandId,
		causationId: metadata.causationId,
		authority,
		resolve,
		reject,
		promise,
		prepared,
		metadata,
		settled: false
	};
	return controller;
}

/**
 * Generated fact commands converge without application polling. Durable status
 * is the only completion signal; timers merely schedule reads and never infer a
 * successful outcome.
 */
function monitorPendingProjection(
	controller: PendingProjection,
	readStatus: () => Promise<ReplicaCommandStatus>,
	retained: () => boolean,
	reportError: ((error: unknown) => void) | undefined
): void {
	if (
		controller.settled ||
		!retained() ||
		projectionAuthorityAborted(controller)
	) {
		return;
	}
	const monitorAbort = new AbortController();
	const stopMonitor = () => monitorAbort.abort();
	controller.stopMonitor = stopMonitor;
	const signals =
		controller.authority.signal === undefined
			? [monitorAbort.signal]
			: [controller.authority.signal, monitorAbort.signal];
	void (async () => {
		try {
			let delay = INITIAL_STATUS_POLL_MS;
			while (
				!controller.settled &&
				retained() &&
				!projectionAuthorityAborted(controller)
			) {
				await waitForProjectionPoll(delay, signals);
				if (
					controller.settled ||
					!retained() ||
					projectionAuthorityAborted(controller)
				) {
					return;
				}
				try {
					await readStatus();
				} catch (error) {
					if (
						controller.settled ||
						!retained() ||
						projectionAuthorityAborted(controller)
					) {
						return;
					}
					reportBackgroundErrorSafely(reportError, error);
				}
				delay = Math.min(delay * 2, MAX_STATUS_POLL_MS);
			}
		} finally {
			if (controller.stopMonitor === stopMonitor) {
				controller.stopMonitor = undefined;
			}
			monitorAbort.abort();
		}
	})().catch((error: unknown) =>
		reportBackgroundErrorSafely(reportError, error)
	);
}

function reportBackgroundErrorSafely(
	reportError: ((error: unknown) => void) | undefined,
	error: unknown
): void {
	if (reportError === undefined) return;
	try {
		reportError(error);
	} catch {
		// Error reporting is a terminal boundary and must never reject detached work.
	}
}

function projectionAuthorityAborted(
	controller: PendingProjection
): boolean {
	return controller.authority.signal?.aborted === true;
}

function waitForProjectionPoll(
	delay: number,
	signals: readonly AbortSignal[]
): Promise<void> {
	if (signals.some((signal) => signal.aborted)) return Promise.resolve();
	return new Promise((resolve) => {
		let settled = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const finish = () => {
			if (settled) return;
			settled = true;
			if (timer !== undefined) clearTimeout(timer);
			for (const signal of signals) {
				signal.removeEventListener('abort', finish);
			}
			resolve();
		};
		timer = setTimeout(finish, delay);
		(
			timer as unknown as {
				unref?: () => void;
			}
		).unref?.();
		for (const signal of signals) {
			signal.addEventListener('abort', finish, { once: true });
		}
		// Close the check/register race if either signal aborted synchronously.
		if (signals.some((signal) => signal.aborted)) finish();
	});
}

function settleProjectionSuccess(controller: PendingProjection): void {
	if (controller.settled) return;
	controller.settled = true;
	controller.abort?.();
	controller.stopMonitor?.();
	controller.resolve(
		Object.freeze({
			commandId: controller.commandId,
			state: 'projected',
			metadata: controller.metadata
		})
	);
}

function callerProjectedPromise(
	controller: PendingProjection,
	signal: AbortSignal | undefined
): Promise<ReplicaCommandProjectedOutcome<unknown>> {
	if (signal === undefined) return controller.promise;
	const callerSignal = signal;
	const promise = new Promise<ReplicaCommandProjectedOutcome<unknown>>(
		(resolve, reject) => {
			let settled = false;
			function settle(complete: () => void): void {
				if (settled) return;
				settled = true;
				callerSignal.removeEventListener('abort', onAbort);
				complete();
			}
			function onAbort(): void {
				/*
				 * Internal causal settlement wins once selected, even if its
				 * promise callbacks have not run yet. Caller cancellation never
				 * mutates that internal lifecycle.
				 */
				if (controller.settled) return;
				settle(() =>
					reject(
						new ReplicaCommandRuntimeError('REPLICA_COMMAND_ABORTED', {
							commandId: controller.commandId
						})
					)
				);
			}
			callerSignal.addEventListener('abort', onAbort, { once: true });
			void controller.promise.then(
				(value) => settle(() => resolve(value)),
				(error: unknown) => settle(() => reject(error))
			);
			if (callerSignal.aborted) onAbort();
		}
	);
	/*
	 * An AbortSignal can fire during an async `onAccepted` callback, before the
	 * receipt reaches caller code. Keep that legitimate rejection observable
	 * without creating a process-level unhandled-rejection race.
	 */
	void promise.catch(() => undefined);
	return promise;
}

function attachAuthorityAbort(
	controller: PendingProjection,
	onSettled: () => void
): void {
	const signal = controller.authority.signal;
	if (signal === undefined) return;
	const onAbort = () => {
		settleProjectionFailure(
			controller,
			new ReplicaCommandRuntimeError(
				'REPLICA_COMMAND_SCOPE_INVALIDATED',
				{ commandId: controller.commandId }
			)
		);
		onSettled();
	};
	signal.addEventListener('abort', onAbort, { once: true });
	controller.abort = () => signal.removeEventListener('abort', onAbort);
	if (signal.aborted) onAbort();
}

function settleProjectionFailure(
	controller: PendingProjection,
	error: unknown
): void {
	if (controller.settled) return;
	controller.settled = true;
	controller.abort?.();
	controller.stopMonitor?.();
	controller.reject(error);
}

function settleTrackedProjection(
	tracker: CommandStatusTracker,
	pending: Map<string, PendingProjection>
): void {
	const controller = tracker.pending;
	if (controller === undefined || controller.settled) return;
	settleProjectionSuccess(controller);
	pending.delete(controller.commandId);
}

function failTrackedProjection(
	tracker: CommandStatusTracker,
	pending: Map<string, PendingProjection>,
	error: unknown
): void {
	const controller = tracker.pending;
	if (controller === undefined) return;
	settleProjectionFailure(controller, error);
	pending.delete(controller.commandId);
}

function normalizeRetries(value: number | undefined): number {
	if (value === undefined) return 0;
	if (!Number.isSafeInteger(value) || value < 0 || value > MAX_TRANSPORT_RETRIES) {
		throw new TypeError(
			`transportRetries must be an integer between 0 and ${MAX_TRANSPORT_RETRIES}`
		);
	}
	return value;
}

function cloneScope(scope: ReplicaAuthoritativeScope): ReplicaAuthoritativeScope {
	return Object.freeze({
		protocolVersion: 2,
		schemaHash: scope.schemaHash,
		cacheScope: scope.cacheScope
	});
}

function sameScope(
	left: ReplicaAuthoritativeScope,
	right: ReplicaAuthoritativeScope
): boolean {
	return (
		left.protocolVersion === right.protocolVersion &&
		left.schemaHash === right.schemaHash &&
		left.cacheScope === right.cacheScope
	);
}

function sameSurface(
	left: ReplicaClientSurface | undefined,
	right: ReplicaClientSurface
): boolean {
	if (
		left === undefined ||
		left.kind !== right.kind ||
		left.name !== right.name
	) {
		return false;
	}
	return (
		left.kind === 'role' ||
		(right.kind === 'application' &&
			left.roles.length === right.roles.length &&
			left.roles.every((role, index) => role === right.roles[index]))
	);
}

function cloneSurface(surface: ReplicaClientSurface): ReplicaClientSurface {
	return surface.kind === 'role'
		? Object.freeze({ kind: 'role', name: surface.name })
		: Object.freeze({
				kind: 'application',
				name: surface.name,
				roles: Object.freeze([...surface.roles])
			});
}

function linkAbortSignals(
	signals: readonly (AbortSignal | undefined)[]
): Readonly<{
	signal: AbortSignal | undefined;
	dispose(): void;
}> {
	const sources = [
		...new Set(
			signals.filter(
				(signal): signal is AbortSignal => signal !== undefined
			)
		)
	];
	if (sources.length === 0) {
		return Object.freeze({
			signal: undefined,
			dispose(): void {}
		});
	}
	if (sources.length === 1) {
		return Object.freeze({
			signal: sources[0],
			dispose(): void {}
		});
	}
	const controller = new AbortController();
	const listeners = new Map<AbortSignal, () => void>();
	let disposed = false;
	const dispose = (): void => {
		if (disposed) return;
		disposed = true;
		for (const [source, listener] of listeners) {
			source.removeEventListener('abort', listener);
		}
		listeners.clear();
	};
	const abort = (signal: AbortSignal) => {
		if (!controller.signal.aborted) {
			controller.abort(signal.reason);
		}
		dispose();
	};
	for (const source of sources) {
		if (source.aborted) {
			abort(source);
			break;
		}
		const listener = () => abort(source);
		listeners.set(source, listener);
		source.addEventListener('abort', listener, { once: true });
		// Close the check/register race if a source aborted synchronously.
		if (source.aborted) {
			listener();
			break;
		}
	}
	return Object.freeze({ signal: controller.signal, dispose });
}

function waitForCommandOperation<T>(
	operation: Promise<T> | T,
	signal: AbortSignal | undefined
): Promise<T> {
	const result = Promise.resolve(operation);
	if (signal === undefined) return result;
	return new Promise<T>((resolve, reject) => {
		let settled = false;
		const finish = (complete: () => void): void => {
			if (settled) return;
			settled = true;
			signal.removeEventListener('abort', onAbort);
			complete();
		};
		const onAbort = (): void => {
			finish(() =>
				reject(
					signal.reason ??
						new ReplicaCommandRuntimeError('REPLICA_COMMAND_ABORTED')
				)
			);
		};
		signal.addEventListener('abort', onAbort, { once: true });
		void result.then(
			(value) => finish(() => resolve(value)),
			(error: unknown) => finish(() => reject(error))
		);
		// Close the check/register race if the signal aborted synchronously.
		if (signal.aborted) onAbort();
	});
}

function isPlainRecord(
	value: unknown
): value is Readonly<Record<string, unknown>> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		return false;
	}
	const prototype = Object.getPrototypeOf(value);
	return prototype === Object.prototype || prototype === null;
}

function outputInvalid(path: string): never {
	throw new ReplicaCommandRuntimeError('REPLICA_COMMAND_PROTOCOL_INVALID', {
		cause: new TypeError(`invalid command output at ${path}`)
	});
}

function compareCodeUnits(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

function comparePropertyKeys(left: PropertyKey, right: PropertyKey): number {
	return compareCodeUnits(String(left), String(right));
}

function commandRuntimeErrorMessage(
	code: ReplicaCommandRuntimeErrorCode
): string {
	switch (code) {
		case 'REPLICA_COMMAND_ABORTED':
			return 'Caller aborted command dispatch or projection visibility wait';
		case 'REPLICA_COMMAND_AUTHORITY_UNAVAILABLE':
			return 'Command dispatch requires a current authoritative replica scope';
		case 'REPLICA_COMMAND_DISPOSED':
			return 'Command runtime is disposed';
		case 'REPLICA_COMMAND_OUTCOME_PENDING':
			return 'Command outcome remains pending';
		case 'REPLICA_COMMAND_PROJECTION_FAILED':
			return 'Command projection failed';
		case 'REPLICA_COMMAND_PROTOCOL_INVALID':
			return 'Command response violated the generated protocol contract';
		case 'REPLICA_COMMAND_REJECTED':
			return 'Command was rejected';
			case 'REPLICA_COMMAND_SCOPE_INVALIDATED':
				return 'Command authorization scope changed';
			case 'REPLICA_COMMAND_STATUS_UNAVAILABLE':
				return 'Generated command status transport is unavailable';
			case 'REPLICA_COMMAND_TRANSPORT_AMBIGUOUS':
				return 'Command transport outcome is ambiguous';
		}
}
