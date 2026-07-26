use crate::client_compiler::manifest::{validate_exact_operation_hash, ManifestProtocolOperations};
use crate::client_compiler::ClientCompileError;

use super::support::invalid;

const COMMAND_STATUS_OPERATION: &str =
    "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";

pub(super) fn validate_protocol_operations(
    operations: &ManifestProtocolOperations,
    commands_present: bool,
) -> Result<(), ClientCompileError> {
    if operations.version != 1 {
        return Err(invalid(
            "client.manifest.protocol_operations",
            "manifest.protocol_operations.version must be 1",
        ));
    }
    match (&operations.command_status, commands_present) {
        (None, true) => Err(invalid(
            "client.manifest.command_status",
            "manifest commands require the framework command-status operation",
        )),
        (Some(_), false) => Err(invalid(
            "client.manifest.command_status",
            "query-only manifests must not expose a command-status operation",
        )),
        (None, false) => Ok(()),
        (Some(status), true) => {
            if status.name != "Distributed_CommandStatus"
                || status.operation != COMMAND_STATUS_OPERATION
            {
                return Err(invalid(
                    "client.manifest.command_status",
                    "command-status operation does not byte-match the framework contract",
                ));
            }
            validate_exact_operation_hash(
                &status.operation,
                &status.operation_hash,
                "command status",
            )
        }
    }
}
