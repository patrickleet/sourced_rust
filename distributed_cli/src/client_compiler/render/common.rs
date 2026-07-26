use super::super::ClientCompileError;

pub(super) fn quoted_property(value: &str) -> String {
    // GraphQL response keys are valid identifiers, but quoting them prevents
    // TypeScript keyword collisions without inventing a second public name.
    serde_json::to_string(value).expect("string serialization cannot fail")
}

pub(super) fn json_string(value: &str) -> Result<String, ClientCompileError> {
    serde_json::to_string(value).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.string",
            format!("failed to render generated string literal: {error}"),
        )
    })
}
