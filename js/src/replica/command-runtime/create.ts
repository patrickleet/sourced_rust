import {
	parseGraphqlResponseExtensions,
	type DistributedProtocolEnvelope
} from '../../protocol.js';
import {
	matchReplicaTrustedPresetInventory,
	prepareReplicaCommandWithTrustedPresets,
	verifyReplicaCommandReceipt,
	type ReplicaCommandArtifact,
	type ReplicaPreparedCommand
} from '../commands.js';
import type { ReplicaResultEnvelope } from '../types.js';
import {
	applyOptimisticEffects,
	attachAuthorityAbort,
	callerProjectedPromise,
	cloneScope,
	commandOutput,
	commandStatusArtifact,
	commandStatusRequest,
	commandSurfaceContract,
	commandTransportRequest,
	confirmDirectProjection,
	defineBoundCommand,
	dispatchPrepared,
	failTrackedProjection,
	freezeCommandTree,
	graphqlCommandRejection,
	linkAbortSignals,
	monitorPendingProjection,
	normalizeInventory,
	normalizeRetries,
	pendingProjection,
	preparedSemanticChanges,
	requireCommandEnvelope,
	requireCommandRejectionEnvelope,
	requireStatusEnvelope,
	sameScope,
	settleProjectionFailure,
	settleProjectionSuccess,
	settleTrackedProjection,
	validateStatusProgression,
	waitForCommandOperation
} from './lib/index.js';
import { ReplicaCommandRuntimeError } from './errors.js';
import {
	replicaCommandAuthority,
	replicaCommandProjectedLifecycle,
	replicaResultObservation
} from './symbols.js';
import type {
	CapturedAuthority,
	CommandEntry,
	CommandStatusTracker,
	PendingProjection,
	ReplicaBoundCommands,
	ReplicaCommandAuthorityHost,
	ReplicaCommandAuthoritySnapshot,
	ReplicaCommandCallOptions,
	ReplicaCommandProjectedOutcome,
	ReplicaCommandReceipt,
	ReplicaCommandRecoveryReceipt,
	ReplicaCommandRuntime,
	ReplicaCommandRuntimeOptions,
	ReplicaCommandStatus,
	ReplicaCommandTransport,
	ReplicaCommandTransportResult,
	SemanticReplica
} from './types.js';

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
				case 'succeeded':
				case 'succeeded_pending_projection':
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
						(metadata.state === 'succeeded' &&
							prepared.confirmations === undefined &&
							prepared.revalidation.required)
					) {
						/*
						 * Status is causal truth, but it carries no canonical
						 * query payload. `projected` fences asynchronous
						 * projection; `succeeded` is terminal when the generated
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

		const rejection = graphqlCommandRejection(result);
		if (rejection !== undefined) {
			try {
				const rejectionEnvelope = requireCommandRejectionEnvelope(
					result,
					prepared,
					authority
				);
				matchReplicaTrustedPresetInventory(
					contract.trustedPresets,
					rejectionEnvelope.trustedPresets
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
			rejectUnmanagedLayer(prepared.commandId);
			throw new ReplicaCommandRuntimeError('REPLICA_COMMAND_REJECTED', {
				commandId: prepared.commandId,
				cause: new Error(rejection.message)
			});
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
			await callOptions.onSucceeded?.(receipt);
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
