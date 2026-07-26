/**
 * Deliver an AggregateError to a user reporter without letting the reporter
 * change success semantics of the surrounding operation.
 */
export function reportSafely(
	reporter: (error: AggregateError) => void,
	error: AggregateError,
	failureMessage = 'error reporter failed'
): void {
	try {
		reporter(error);
	} catch (reporterError) {
		queueMicrotask(() => {
			throw new AggregateError([error, reporterError], failureMessage);
		});
	}
}

/** Best-effort delivery of an unhandled AggregateError to the host. */
export function reportUnhandledError(error: AggregateError): void {
	const reportError = (globalThis as { reportError?: (cause: unknown) => void })
		.reportError;
	if (typeof reportError === 'function') {
		reportError(error);
		return;
	}
	queueMicrotask(() => {
		throw error;
	});
}
