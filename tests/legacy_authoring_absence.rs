//! Structural gate: competing projector authoring surfaces must stay gone.
//!
//! Checks that `projection!`, separately authored `command_effects!` /
//! `command_confirmations!`, command-side `.project(...)` selectors, and the
//! public `ProjectionReadModelWorkspace` ORM authoring path stay removed.

#![cfg(feature = "graphql")]

#[test]
fn projection_macro_is_not_in_the_public_prelude() {
    let _ = stringify!(mutation);
    let lib = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));
    assert!(
        !lib.contains("projection,"),
        "event-owning projection! must not be re-exported at crate root"
    );
}

#[test]
fn mutation_macro_is_the_public_authoring_export() {
    let lib = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));
    assert!(
        lib.contains("mutation,"),
        "mutation! must remain publicly re-exported"
    );
    // Reject live re-exports (comments may still mention the removed macros).
    assert!(
        !lib.contains("command_effects,")
            && !lib.contains("command_effects }")
            && !lib.lines().any(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && trimmed.contains("command_effects")
            }),
        "command_effects! must not be re-exported at crate root"
    );
    assert!(
        !lib.contains("command_confirmations,")
            && !lib.contains("command_confirmations }")
            && !lib.lines().any(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && trimmed.contains("command_confirmations")
            }),
        "command_confirmations! must not be re-exported at crate root"
    );
}

#[test]
fn handlers_source_has_no_command_side_project_selector() {
    let handlers = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/microsvc/service/handlers.rs"
    ));
    assert!(
        !handlers.contains("pub fn project<"),
        "command-side .project(...) selector must be removed from handlers.rs"
    );
}

#[test]
fn projection_read_model_workspace_is_not_publicly_exported() {
    let microsvc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/microsvc/mod.rs"));
    assert!(
        !microsvc.contains("ProjectionReadModelWorkspace"),
        "ProjectionReadModelWorkspace must not be re-exported from microsvc"
    );
    let projector_mod = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/microsvc/projector/mod.rs"
    ));
    assert!(
        !projector_mod.contains("pub use graph_workspace")
            && !projector_mod.contains("ProjectionReadModelWorkspace,"),
        "graph workspace must not be re-exported from projector mod"
    );
}

#[test]
fn typed_command_has_no_public_effects_or_confirmations_builder() {
    let typed = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/graphql/command_contract/typed_command.rs"
    ));
    assert!(
        !typed.contains("pub fn effects("),
        "TypedCommand::effects must be removed"
    );
    assert!(
        !typed.contains("pub fn confirmations("),
        "TypedCommand::confirmations must be removed"
    );
}

#[test]
fn macros_crate_does_not_export_effects_or_projection_macros() {
    let macros_lib = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/distributed_macros/src/lib.rs"
    ));
    assert!(
        !macros_lib.contains("pub fn command_effects("),
        "command_effects! proc-macro must be removed"
    );
    assert!(
        !macros_lib.contains("pub fn command_confirmations("),
        "command_confirmations! proc-macro must be removed"
    );
    assert!(
        !macros_lib.contains("pub fn projection("),
        "projection! proc-macro must stay removed"
    );
}
