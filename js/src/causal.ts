import { documentToString, type GqlDocument } from './document.js';
import {
	DISTRIBUTED_PROTOCOL_VERSION,
	DistributedProtocolError,
	parseGraphqlResponseExtensions,
	type DistributedCommandMetadata,
	type DistributedCommandState,
	type DistributedOpaqueString,
	type DistributedProtocolEnvelope
} from './protocol.js';
import type { GqlError, GqlResult, GraphqlVariables } from './types.js';

const SHA256 = /^sha256:[0-9a-f]{64}$/;
const UUID_V7 =
	/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const MAX_OPERATION_BYTES = 1024 * 1024;
const MAX_STATUS_ATTEMPTS = 256;
const MAX_TRANSPORT_RETRIES = 8;
const MAX_DELAY_MS = 30_000;
const MAX_DEADLINE_MS = 5 * 60_000;

/** Exact framework operation emitted by a role-selected client manifest. */
export type CausalStatusOperation = Readonly<{
	name: string;
	document: GqlDocument;
	operationHash: string;
}>;

/**
 * Causal protocol material generated from the same role-selected manifest as
 * the command documents. The runtime never builds a status query itself.
 */
export type CausalCommandProtocol = Readonly<{
	protocolVersion: typeof DISTRIBUTED_PROTOCOL_VERSION;
	schemaHash: string;
	commandStatus: CausalStatusOperation;
}>;

/** Per-command causal material emitted by client-manifest code generation. */
export type CausalCommandDefinition = Readonly<{
	protocol: CausalCommandProtocol;
	operationHash: string;
	/** False for terminal Accepted commands with no finite confirmation plan. */
	projects: boolean;
}>;

export type CausalRecoveryOptions = Readonly<{
	/** Retries of the exact mutation after a thrown/ambiguous transport call. */
	transportRetries?: number;
	/** Maximum status requests made by one projected wait. */
	maxStatusAttempts?: number;
	/** Total projected-wait budget, including backoff. */
	deadlineMs?: number;
	initialDelayMs?: number;
	maxDelayMs?: number;
	backoffFactor?: number;
	signal?: AbortSignal;
}>;

export type CausalCommandCallOptions = Readonly<{
	/** Supply one stable UUIDv7 when coordinating duplicate submissions. */
	commandId?: string;
	recovery?: CausalRecoveryOptions;
}>;

export type CausalCommandStatus = Readonly<{
	commandId: string;
	state: DistributedCommandState;
	metadata?: DistributedCommandMetadata;
}>;

export type CausalProjectedOutcome = Readonly<{
	commandId: string;
	state: 'projected';
	metadata?: DistributedCommandMetadata;
}>;

export type CausalReceiptErrorCode =
	| 'CAUSAL_COMMAND_REJECTED'
	| 'CAUSAL_PROJECTION_FAILED'
	| 'CAUSAL_COMMAND_EXPIRED'
	| 'CAUSAL_SCOPE_INVALIDATED'
	| 'CAUSAL_SCHEMA_INVALIDATED'
	| 'CAUSAL_PROTOCOL_INVALID'
	| 'CAUSAL_DEADLINE_EXCEEDED'
	| 'CAUSAL_ABORTED'
	| 'CAUSAL_PROJECTION_UNAVAILABLE';

/** Typed, redacted failure from receipt recovery or projected visibility. */
export class CausalReceiptError extends Error {
	readonly code: CausalReceiptErrorCode;
	readonly commandId: string;
	readonly state: DistributedCommandState;

	constructor(
		code: CausalReceiptErrorCode,
		commandId: string,
		state: DistributedCommandState
	) {
		super(causalErrorMessage(code));
		this.name = 'CausalReceiptError';
		this.code = code;
		this.commandId = commandId;
		this.state = state;
	}
}

type CausalRequestClient = {
	request: <
		TData = Record<string, unknown>,
		TVariables extends GraphqlVariables = GraphqlVariables
	>(
		document: GqlDocument<TData, TVariables>,
		variables?: TVariables,
		options?: { cache?: 'skip' }
	) => Promise<GqlResult<TData>>;
};

type NormalizedRecovery = Readonly<{
	transportRetries: number;
	maxStatusAttempts: number;
	deadlineMs: number;
	initialDelayMs: number;
	maxDelayMs: number;
	backoffFactor: number;
	signal?: AbortSignal;
}>;

/**
 * Causal command handle. `projected` is deliberately absent for Accepted
 * commands whose manifest declares no finite projection visibility contract.
 */
export class CausalCommandReceipt {
	readonly commandId: string;
	readonly #client: CausalRequestClient;
	readonly #definition: CausalCommandDefinition;
	readonly #recovery: NormalizedRecovery;
	#state: DistributedCommandState = 'unknown';
	#metadata: DistributedCommandMetadata | undefined;
	#cacheScope: DistributedOpaqueString | undefined;
	#causationId: DistributedOpaqueString | undefined;
	#consistency: DistributedCommandMetadata['consistency'] | undefined;
	#expectationsFingerprint: string | undefined;
	#correlationFailure: CausalReceiptError | undefined;
	#projectedPromise: Promise<CausalProjectedOutcome> | undefined;
	#statusInFlight: Promise<CausalCommandStatus> | undefined;
	#statusConsumers = 0;
	#nextStatusAttempt = 0;
	#latestAppliedStatusAttempt = 0;

	constructor(
		client: CausalRequestClient,
		definition: CausalCommandDefinition,
		commandId: string,
		recovery: CausalRecoveryOptions = {}
	) {
		this.#client = client;
		this.#definition = definition;
		this.commandId = validateCommandId(commandId);
		this.#recovery = normalizeRecovery(recovery);
	}

	get state(): DistributedCommandState {
		return this.#state;
	}

	get metadata(): DistributedCommandMetadata | undefined {
		return this.#metadata;
	}

	/**
	 * Lazily shared visibility wait. Multiple consumers of one handle never
	 * start duplicate poll loops. Separate tabs may poll independently; the
	 * server's principal-partitioned ledger remains authoritative.
	 */
	get projected(): Promise<CausalProjectedOutcome> | undefined {
		if (!this.#definition.projects) return undefined;
		if (!this.#projectedPromise) {
			this.#projectedPromise = this.waitForProjected();
		}
		return this.#projectedPromise;
	}

	/** Perform one generated status operation and return its typed state. */
	status(options: Pick<CausalRecoveryOptions, 'signal'> = {}): Promise<CausalCommandStatus> {
		if (this.#statusInFlight) return this.#statusInFlight;
		const signal = options.signal ?? this.#recovery.signal;
		const attempt = ++this.#nextStatusAttempt;
		let tracked: Promise<CausalCommandStatus>;
		tracked = this.#readStatus(signal, attempt).finally(() => {
			if (this.#statusInFlight === tracked) this.#statusInFlight = undefined;
		});
		this.#statusInFlight = tracked;
		return tracked;
	}

	/** Wait with bounded backoff until projection succeeds or terminates. */
	async waitForProjected(
		options: CausalRecoveryOptions = {}
	): Promise<CausalProjectedOutcome> {
		if (!this.#definition.projects) {
			throw this.#error('CAUSAL_PROJECTION_UNAVAILABLE');
		}
		const recovery = normalizeRecovery({ ...this.#recovery, ...options });
		const startedAt = Date.now();
		let delayMs = recovery.initialDelayMs;

		for (let attempt = 0; attempt < recovery.maxStatusAttempts; attempt += 1) {
			throwIfAborted(recovery.signal, this.commandId, this.#state);
			const terminal = this.#projectedOrTerminal();
			if (terminal) return terminal;
			if (Date.now() - startedAt >= recovery.deadlineMs) {
				throw this.#error('CAUSAL_DEADLINE_EXCEEDED');
			}

			if (attempt > 0 || this.#metadata !== undefined) {
				await abortableDelay(
					Math.min(delayMs, remaining(startedAt, recovery.deadlineMs)),
					recovery.signal,
					this.commandId,
					this.#state
				);
				delayMs = Math.min(
					recovery.maxDelayMs,
					Math.max(1, Math.ceil(delayMs * recovery.backoffFactor))
				);
			}
			if (Date.now() - startedAt >= recovery.deadlineMs) {
				throw this.#error('CAUSAL_DEADLINE_EXCEEDED');
			}

			try {
				const statusRequest = this.status();
				this.#statusConsumers += 1;
				let abandoned = false;
				try {
					await awaitWithinDeadline(
						statusRequest,
						remaining(startedAt, recovery.deadlineMs),
						recovery.signal,
						this.commandId,
						this.#state
					);
				} catch (error) {
					abandoned =
						error instanceof CausalReceiptError &&
						(error.code === 'CAUSAL_ABORTED' ||
							error.code === 'CAUSAL_DEADLINE_EXCEEDED');
					throw error;
				} finally {
					this.#statusConsumers -= 1;
					if (
						abandoned &&
						this.#statusConsumers === 0 &&
						this.#statusInFlight === statusRequest
					) {
						this.#statusInFlight = undefined;
					}
				}
				const afterStatus = this.#projectedOrTerminal();
				if (afterStatus) return afterStatus;
			} catch (error) {
				if (error instanceof CausalReceiptError) throw error;
				// A thrown status transport is ambiguous and retryable until the
				// caller's explicit bound. It never changes the known state.
			}
		}

		throw this.#error('CAUSAL_DEADLINE_EXCEEDED');
	}

	/** @internal Validate and retain one strict command response envelope. */
	observeCommandResult(result: GqlResult<unknown>): void {
		const envelope = this.#envelope(
			result,
			this.#definition.operationHash,
			true
		);
		this.#observeCommand(envelope.command!);
	}

	/** @internal Record a thrown mutation only by retaining explicit unknown. */
	observeAmbiguousTransport(): void {
		if (this.#metadata === undefined) this.#state = 'unknown';
	}

	/** @internal Map transport-level Distributed parser failures to this receipt. */
	protocolFailure(): CausalReceiptError {
		return this.#fatal('CAUSAL_PROTOCOL_INVALID');
	}

	/** @internal Exact same-ID mutation retry count. */
	get transportRetries(): number {
		return this.#recovery.transportRetries;
	}

	/** @internal Signal shared by mutation retry and default projected wait. */
	get signal(): AbortSignal | undefined {
		return this.#recovery.signal;
	}

	/** @internal Mutation retry delay using the configured bounded backoff. */
	async retryDelay(retryIndex: number): Promise<void> {
		const delay = Math.min(
			this.#recovery.maxDelayMs,
			Math.ceil(
				this.#recovery.initialDelayMs *
					this.#recovery.backoffFactor ** retryIndex
			)
		);
		await abortableDelay(
			delay,
			this.#recovery.signal,
			this.commandId,
			this.#state
		);
	}

	async #readStatus(
		signal: AbortSignal | undefined,
		attempt: number
	): Promise<CausalCommandStatus> {
		throwIfAborted(signal, this.commandId, this.#state);
		const operation = this.#definition.protocol.commandStatus;
		const result = await this.#client.request<
			{ commandStatus?: { state?: unknown } | null },
			{ commandId: string }
		>(
			operation.document,
			{ commandId: this.commandId },
			{ cache: 'skip' }
		);
		if (attempt < this.#latestAppliedStatusAttempt) {
			return this.#currentStatus();
		}
		const hasEnvelope = this.#hasDistributedEnvelope(result);
		if (
			!hasEnvelope &&
			((result.errors?.length ?? 0) > 0 || result.status >= 500)
		) {
			throw this.#statusFailure(result);
		}
		const envelope = this.#envelope(result, operation.operationHash, false);
		if (
			((result.errors?.length ?? 0) > 0 || result.status >= 500) &&
			envelope.command === undefined
		) {
			throw this.#statusFailure(result);
		}
		let state: DistributedCommandState;
		try {
			state = parseStatusState(result.data?.commandStatus?.state);
		} catch {
			if (
				((result.errors?.length ?? 0) > 0 || result.status >= 500) &&
				envelope.command !== undefined
			) {
				state = envelope.command.state;
			} else {
				throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
			}
		}
		this.#latestAppliedStatusAttempt = attempt;
		let effectiveState: DistributedCommandState;
		if (envelope.command) {
			if (envelope.command.state !== state) {
				throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
			}
			effectiveState = this.#observeCommand(envelope.command);
		} else {
			effectiveState = this.#observeState(state);
		}
		return Object.freeze({
			commandId: this.commandId,
			state: effectiveState,
			...(envelope.command && this.#metadata
				? { metadata: this.#metadata }
				: {})
		});
	}

	#observeCommand(
		command: DistributedCommandMetadata
	): DistributedCommandState {
		if (command.commandId !== this.commandId) {
			throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		if (
			this.#causationId !== undefined &&
			command.causationId !== this.#causationId
		) {
			throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		if (
			this.#consistency !== undefined &&
			command.consistency !== this.#consistency
		) {
			throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		if (
			isTerminalCommandState(this.#state, this.#definition.projects) &&
			command.state !== this.#state
		) {
			return this.#state;
		}

		let expectations: string;
		try {
			expectations = JSON.stringify(command.expects);
		} catch {
			throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		const provisionalEmpty =
			this.#expectationsFingerprint === undefined &&
			command.state === 'in_progress' &&
			command.expects.length === 0;
		if (!provisionalEmpty) {
			if (
				this.#expectationsFingerprint !== undefined &&
				expectations !== this.#expectationsFingerprint
			) {
				throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
			}
			this.#expectationsFingerprint = expectations;
		}

		this.#causationId = command.causationId;
		this.#consistency = command.consistency;
		const state = this.#observeState(command.state);
		this.#metadata = command;
		return state;
	}

	#observeState(state: DistributedCommandState): DistributedCommandState {
		if (isTerminalCommandState(this.#state, this.#definition.projects)) {
			return this.#state;
		}
		if (this.#definition.projects && state === 'accepted') {
			this.#state = state;
			throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		this.#state = state;
		return this.#state;
	}

	#statusFailure(result: GqlResult<unknown>): Error {
		const errors = result.errors ?? [];
		const codes = new Set(errors.map((error) => error.extensions?.code));
		if (
			result.status === 401 ||
			result.status === 403 ||
			codes.has('UNAUTHORIZED') ||
			codes.has('FORBIDDEN')
		) {
			return this.#fatal('CAUSAL_SCOPE_INVALIDATED');
		}
		if (
			codes.has('BAD_REQUEST') ||
			codes.has('COMMAND_ID_REUSE') ||
			codes.has('REJECTED') ||
			result.status === 404 ||
			codes.has('NOT_FOUND')
		) {
			return this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		if (
			result.status >= 500 ||
			codes.has('INTERNAL') ||
			codes.has('TIMEOUT')
		) {
			return new Error('causal command status is temporarily unavailable');
		}
		return this.#fatal('CAUSAL_PROTOCOL_INVALID');
	}

	#hasDistributedEnvelope(result: GqlResult<unknown>): boolean {
		try {
			return (
				parseGraphqlResponseExtensions(result.extensions)?.distributed !==
				undefined
			);
		} catch (error) {
			if (error instanceof DistributedProtocolError) {
				throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
			}
			throw error;
		}
	}

	#envelope(
		result: GqlResult<unknown>,
		expectedOperation: string,
		requireCommand: boolean
	): DistributedProtocolEnvelope {
		let extensions;
		try {
			extensions = parseGraphqlResponseExtensions(result.extensions);
		} catch (error) {
			if (error instanceof DistributedProtocolError) {
				throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
			}
			throw error;
		}
		const envelope = extensions?.distributed;
		if (
			!envelope ||
			envelope.schemaHash !== this.#definition.protocol.schemaHash ||
			envelope.operation !== expectedOperation ||
			(requireCommand && envelope.command === undefined)
		) {
			if (
				envelope &&
				envelope.schemaHash !== this.#definition.protocol.schemaHash
			) {
				throw this.#fatal('CAUSAL_SCHEMA_INVALIDATED');
			}
			throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
		if (
			this.#cacheScope !== undefined &&
			envelope.cacheScope !== this.#cacheScope
		) {
			throw this.#fatal('CAUSAL_SCOPE_INVALIDATED');
		}
		this.#cacheScope = envelope.cacheScope;
		return envelope;
	}

	#projectedOrTerminal(): CausalProjectedOutcome | undefined {
		if (this.#correlationFailure) throw this.#correlationFailure;
		switch (this.#state) {
			case 'projected':
				return Object.freeze({
					commandId: this.commandId,
					state: 'projected',
					...(this.#metadata ? { metadata: this.#metadata } : {})
				});
			case 'rejected':
				throw this.#error('CAUSAL_COMMAND_REJECTED');
			case 'projection_failed':
				throw this.#error('CAUSAL_PROJECTION_FAILED');
			case 'expired':
				throw this.#error('CAUSAL_COMMAND_EXPIRED');
			case 'in_progress':
			case 'accepted_pending_projection':
			case 'unknown':
				return undefined;
			case 'accepted':
				throw this.#fatal('CAUSAL_PROTOCOL_INVALID');
		}
	}

	#currentStatus(): CausalCommandStatus {
		return Object.freeze({
			commandId: this.commandId,
			state: this.#state,
			...(this.#metadata ? { metadata: this.#metadata } : {})
		});
	}

	#error(code: CausalReceiptErrorCode): CausalReceiptError {
		return new CausalReceiptError(code, this.commandId, this.#state);
	}

	#fatal(code: CausalReceiptErrorCode): CausalReceiptError {
		if (!this.#correlationFailure) {
			this.#correlationFailure = this.#error(code);
		}
		return this.#correlationFailure;
	}
}

/** Validate and freeze generated protocol material. */
export function defineCausalProtocol(
	protocol: CausalCommandProtocol
): CausalCommandProtocol {
	if (protocol.protocolVersion !== DISTRIBUTED_PROTOCOL_VERSION) {
		throw new Error('causal protocol version is unsupported');
	}
	validateHash(protocol.schemaHash, 'causal schema hash');
	const status = protocol.commandStatus;
	if (!status.name.trim()) throw new Error('causal status operation name must not be empty');
	validateDocument(status.document, 'causal status operation');
	validateHash(status.operationHash, 'causal status operation hash');
	return Object.freeze({
		protocolVersion: DISTRIBUTED_PROTOCOL_VERSION,
		schemaHash: protocol.schemaHash,
		commandStatus: Object.freeze({ ...status })
	});
}

/** Create a browser/Node UUIDv7 command identity. */
export function createCommandId(): string {
	const crypto = globalThis.crypto;
	if (!crypto || typeof crypto.getRandomValues !== 'function') {
		throw new Error('causal commands require crypto.getRandomValues');
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

/** @internal Prepare one causal command execution before optimism is applied. */
export function createCausalCommandReceipt(
	client: CausalRequestClient,
	definition: CausalCommandDefinition,
	options: CausalCommandCallOptions = {}
): CausalCommandReceipt {
	validateHash(definition.operationHash, 'causal command operation hash');
	const commandId = options.commandId ?? createCommandId();
	return new CausalCommandReceipt(
		client,
		definition,
		commandId,
		options.recovery
	);
}

/** @internal Retry one exact causal mutation without changing its variables. */
export async function requestCausalCommand<
	TData,
	TVariables extends GraphqlVariables
>(
	client: CausalRequestClient,
	receipt: CausalCommandReceipt,
	document: GqlDocument<TData, TVariables>,
	variables: TVariables
): Promise<GqlResult<TData>> {
	const stableVariables = cloneWireVariables(variables) as TVariables;
	let lastError: unknown;
	for (
		let attempt = 0;
		attempt <= receipt.transportRetries;
		attempt += 1
	) {
		throwIfAborted(receipt.signal, receipt.commandId, receipt.state);
		try {
			const result = await client.request(document, stableVariables);
			let envelope: DistributedProtocolEnvelope | undefined;
			try {
				envelope = parseGraphqlResponseExtensions(
					result.extensions
				)?.distributed;
			} catch (error) {
				if (error instanceof DistributedProtocolError) {
					throw receipt.protocolFailure();
				}
				throw error;
			}
			if (envelope?.command !== undefined) {
				receipt.observeCommandResult(result);
				return result;
			}
			if (result.errors?.length) {
				if (
					result.errors.some(
						(error) =>
							error.extensions?.code === 'DISTRIBUTED_PROTOCOL_INVALID' ||
							error.extensions?.code ===
								'DISTRIBUTED_PROTOCOL_VERSION_UNSUPPORTED'
					)
				) {
					throw receipt.protocolFailure();
				}
				const codes = new Set(
					result.errors.map((error) => error.extensions?.code)
				);
				if (
					codes.has('REJECTED') ||
					codes.has('COMMAND_ID_REUSE') ||
					codes.has('UNAUTHORIZED') ||
					codes.has('FORBIDDEN') ||
					codes.has('BAD_REQUEST')
				) {
					return result;
				}
				throw new Error('causal command outcome is ambiguous');
			}
			if (result.status >= 500) {
				throw new Error('causal command outcome is ambiguous');
			}
			receipt.observeCommandResult(result);
			return result;
		} catch (error) {
			if (error instanceof CausalReceiptError) throw error;
			lastError = error;
			receipt.observeAmbiguousTransport();
			if (attempt < receipt.transportRetries) {
				await receipt.retryDelay(attempt);
			}
		}
	}
	throw lastError;
}

/** Convert an ambiguous causal request into the package's result-style error. */
export function causalTransportFailure(
	_error: unknown,
	receipt: CausalCommandReceipt
): GqlResult<never> {
	const item: GqlError = {
		message: 'Command transport outcome is ambiguous',
		extensions: { code: 'CAUSAL_TRANSPORT_AMBIGUOUS' }
	};
	return {
		data: undefined,
		errors: [item],
		status: 0,
		receipt
	};
}

/** Convert a fail-closed receipt error into the package's result-style error. */
export function causalReceiptFailure(
	error: CausalReceiptError,
	receipt: CausalCommandReceipt
): GqlResult<never> {
	return {
		data: undefined,
		errors: [
			{
				message: error.message,
				extensions: { code: error.code }
			}
		],
		status: 0,
		receipt
	};
}

function parseStatusState(value: unknown): DistributedCommandState {
	switch (value) {
		case 'in_progress':
		case 'accepted':
		case 'accepted_pending_projection':
		case 'projected':
		case 'rejected':
		case 'projection_failed':
		case 'expired':
		case 'unknown':
			return value;
		default:
			throw new Error('causal command status returned an invalid state');
	}
}

function isTerminalCommandState(
	state: DistributedCommandState,
	projects: boolean
): boolean {
	return (
		state === 'projected' ||
		state === 'rejected' ||
		state === 'projection_failed' ||
		state === 'expired' ||
		(!projects && state === 'accepted')
	);
}

function cloneWireVariables(
	variables: GraphqlVariables
): GraphqlVariables {
	return cloneWireObject(variables, new Set<object>());
}

function cloneWireObject(
	value: Record<string, unknown>,
	ancestors: Set<object>
): GraphqlVariables {
	if (ancestors.has(value)) {
		throw new TypeError('causal command variables must not contain cycles');
	}
	ancestors.add(value);
	const clone: Record<string, unknown> = {};
	for (const [key, item] of Object.entries(value)) {
		if (item === undefined) continue;
		Object.defineProperty(clone, key, {
			value: cloneWireValue(item, ancestors),
			enumerable: true,
			configurable: false,
			writable: false
		});
	}
	ancestors.delete(value);
	return Object.freeze(clone);
}

function cloneWireValue(
	value: unknown,
	ancestors: Set<object>
): unknown {
	if (
		value === null ||
		typeof value === 'string' ||
		typeof value === 'boolean'
	) {
		return value;
	}
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) {
			throw new TypeError(
				'causal command variables must contain finite numbers'
			);
		}
		return value;
	}
	if (typeof value !== 'object') {
		throw new TypeError(
			'causal command variables must be JSON-compatible'
		);
	}
	if (ancestors.has(value)) {
		throw new TypeError('causal command variables must not contain cycles');
	}
	if (Array.isArray(value)) {
		ancestors.add(value);
		const clone = value.map((item) => cloneWireValue(item, ancestors));
		ancestors.delete(value);
		return Object.freeze(clone);
	}
	const prototype = Object.getPrototypeOf(value);
	if (prototype !== Object.prototype && prototype !== null) {
		throw new TypeError(
			'causal command variables must contain plain JSON objects'
		);
	}
	return cloneWireObject(
		value as Record<string, unknown>,
		ancestors
	);
}

function normalizeRecovery(options: CausalRecoveryOptions): NormalizedRecovery {
	const transportRetries = integerOption(
		options.transportRetries,
		1,
		0,
		MAX_TRANSPORT_RETRIES,
		'transportRetries'
	);
	const maxStatusAttempts = integerOption(
		options.maxStatusAttempts,
		64,
		1,
		MAX_STATUS_ATTEMPTS,
		'maxStatusAttempts'
	);
	const deadlineMs = integerOption(
		options.deadlineMs,
		30_000,
		1,
		MAX_DEADLINE_MS,
		'deadlineMs'
	);
	const initialDelayMs = integerOption(
		options.initialDelayMs,
		50,
		0,
		MAX_DELAY_MS,
		'initialDelayMs'
	);
	const maxDelayMs = integerOption(
		options.maxDelayMs,
		1_000,
		1,
		MAX_DELAY_MS,
		'maxDelayMs'
	);
	const backoffFactor = options.backoffFactor ?? 1.6;
	if (
		!Number.isFinite(backoffFactor) ||
		backoffFactor < 1 ||
		backoffFactor > 10
	) {
		throw new Error('causal recovery backoffFactor must be between 1 and 10');
	}
	if (initialDelayMs > maxDelayMs) {
		throw new Error('causal recovery initialDelayMs must not exceed maxDelayMs');
	}
	return Object.freeze({
		transportRetries,
		maxStatusAttempts,
		deadlineMs,
		initialDelayMs,
		maxDelayMs,
		backoffFactor,
		...(options.signal ? { signal: options.signal } : {})
	});
}

function integerOption(
	value: number | undefined,
	fallback: number,
	min: number,
	max: number,
	name: string
): number {
	const actual = value ?? fallback;
	if (!Number.isInteger(actual) || actual < min || actual > max) {
		throw new Error(`causal recovery ${name} must be an integer from ${min} to ${max}`);
	}
	return actual;
}

function validateCommandId(value: string): string {
	if (!UUID_V7.test(value)) {
		throw new Error('causal commandId must be an RFC 4122 UUIDv7');
	}
	return value.toLowerCase();
}

function validateHash(value: string, label: string): void {
	if (!SHA256.test(value)) throw new Error(`${label} must be a canonical SHA-256`);
}

function validateDocument(value: GqlDocument, label: string): void {
	let document: string;
	try {
		document = documentToString(value);
	} catch {
		throw new Error(`${label} must contain one bounded generated document`);
	}
	if (
		typeof document !== 'string' ||
		document.trim() === '' ||
		new TextEncoder().encode(document).length > MAX_OPERATION_BYTES
	) {
		throw new Error(`${label} must contain one bounded generated document`);
	}
}

function throwIfAborted(
	signal: AbortSignal | undefined,
	commandId: string,
	state: DistributedCommandState
): void {
	if (signal?.aborted) {
		throw new CausalReceiptError('CAUSAL_ABORTED', commandId, state);
	}
}

function awaitWithinDeadline<T>(
	promise: Promise<T>,
	deadlineMs: number,
	signal: AbortSignal | undefined,
	commandId: string,
	state: DistributedCommandState
): Promise<T> {
	throwIfAborted(signal, commandId, state);
	if (deadlineMs <= 0) {
		return Promise.reject(
			new CausalReceiptError(
				'CAUSAL_DEADLINE_EXCEEDED',
				commandId,
				state
			)
		);
	}
	return new Promise((resolve, reject) => {
		const timer = setTimeout(() => {
			cleanup();
			reject(
				new CausalReceiptError(
					'CAUSAL_DEADLINE_EXCEEDED',
					commandId,
					state
				)
			);
		}, deadlineMs);
		const abort = () => {
			cleanup();
			reject(new CausalReceiptError('CAUSAL_ABORTED', commandId, state));
		};
		const cleanup = () => {
			clearTimeout(timer);
			signal?.removeEventListener('abort', abort);
		};
		signal?.addEventListener('abort', abort, { once: true });
		promise.then(
			(value) => {
				cleanup();
				resolve(value);
			},
			(error: unknown) => {
				cleanup();
				reject(error);
			}
		);
	});
}

function abortableDelay(
	delayMs: number,
	signal: AbortSignal | undefined,
	commandId: string,
	state: DistributedCommandState
): Promise<void> {
	if (delayMs <= 0) {
		throwIfAborted(signal, commandId, state);
		return Promise.resolve();
	}
	return new Promise((resolve, reject) => {
		const timer = setTimeout(done, delayMs);
		const abort = () => {
			clearTimeout(timer);
			signal?.removeEventListener('abort', abort);
			reject(new CausalReceiptError('CAUSAL_ABORTED', commandId, state));
		};
		function done() {
			signal?.removeEventListener('abort', abort);
			resolve();
		}
		signal?.addEventListener('abort', abort, { once: true });
	});
}

function remaining(startedAt: number, deadlineMs: number): number {
	return Math.max(0, deadlineMs - (Date.now() - startedAt));
}

function causalErrorMessage(code: CausalReceiptErrorCode): string {
	switch (code) {
		case 'CAUSAL_COMMAND_REJECTED':
			return 'Command was rejected';
		case 'CAUSAL_PROJECTION_FAILED':
			return 'Command projection failed';
		case 'CAUSAL_COMMAND_EXPIRED':
			return 'Command receipt expired';
		case 'CAUSAL_SCOPE_INVALIDATED':
			return 'Command receipt authorization scope changed';
		case 'CAUSAL_SCHEMA_INVALIDATED':
			return 'Command receipt schema changed';
		case 'CAUSAL_PROTOCOL_INVALID':
			return 'Command receipt protocol response was invalid';
		case 'CAUSAL_DEADLINE_EXCEEDED':
			return 'Command projection deadline exceeded';
		case 'CAUSAL_ABORTED':
			return 'Command projection wait was aborted';
		case 'CAUSAL_PROJECTION_UNAVAILABLE':
			return 'Command has no declared projection visibility contract';
	}
}
