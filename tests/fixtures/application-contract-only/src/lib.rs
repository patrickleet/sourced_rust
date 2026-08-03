use distributed::application::{ApplicationManifest, ContractCompiler};

/// Contract-only packages can materialize portable artifacts without linking
/// a service, repository, runtime, or handler implementation.
pub fn manifest_bytes() -> Vec<u8> {
    ApplicationManifest::new("contract-only")
        .canonical_bytes()
        .expect("contract manifest should be serializable")
}

pub fn surface_sdl() -> String {
    ContractCompiler::new("contract-only")
        .graphql_sdl()
        .expect("contract surface should compile")
}
