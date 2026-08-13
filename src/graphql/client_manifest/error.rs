#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientManifestError(pub String);

impl std::fmt::Display for ClientManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClientManifestError {}

impl From<serde_json::Error> for ClientManifestError {
    fn from(error: serde_json::Error) -> Self {
        Self(format!("client manifest serialization failed: {error}"))
    }
}
