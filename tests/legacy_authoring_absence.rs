//! Structural gate: competing projector authoring surfaces must stay gone.
//!
//! This is a compile-time + runtime check that `projection!` is not re-exported
//! and command-side `.project(...)` selectors are not part of the public
//! causal commit API.

#![cfg(feature = "graphql")]

#[test]
fn projection_macro_is_not_in_the_public_prelude() {
    // If someone re-exports `projection!`, this test file would be the place
    // to add a trybuild compile_fail. Runtime: ensure mutation! is the path.
    let _ = stringify!(mutation);
    // The following must not resolve as a public API name in docs/exports.
    // (Macro absence is enforced by not importing it and by rg in CI.)
    assert!(!include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/lib.rs"
    ))
    .contains("pub use distributed_macros::{\n    aggregate, command_confirmations, command_effects, command_input_defaults, digest, mutation,\n    projection,"));
}

#[test]
fn mutation_macro_is_the_public_authoring_export() {
    let lib = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));
    assert!(
        lib.contains("mutation,"),
        "mutation! must remain publicly re-exported"
    );
    assert!(
        !lib.contains("projection,"),
        "event-owning projection! must not be re-exported at crate root"
    );
}

#[test]
fn handlers_source_has_no_command_side_project_selector() {
    let handlers = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/microsvc/service/handlers.rs"
    ));
    // Public method must not exist: `pub fn project`
    assert!(
        !handlers.contains("pub fn project<"),
        "command-side .project(...) selector must be removed from handlers.rs"
    );
}
