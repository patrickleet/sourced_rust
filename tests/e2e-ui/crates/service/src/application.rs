//! Application composition root for the e2e-ui fixture (task 13).
//!
//! This module is the explicit, reviewable declaration of the e2e application:
//! surface identities and (as modules are fully ported) module lists. Runtime
//! wiring in `service.rs` should shrink toward framework host invocation that
//! consumes these declarations rather than hand-pairing dialect runners.

/// Stable normal-application surface shared by user and admin sessions.
pub const DISTRIBUTED_CLIENT_SURFACE: &str = "e2e-ui";
/// Stable elevated surface for routes that intentionally include admin-only fields.
pub const DISTRIBUTED_ADMIN_CLIENT_SURFACE: &str = "e2e-ui-admin";
/// Unauthenticated public surface (lobby message peek).
pub const DISTRIBUTED_PUBLIC_CLIENT_SURFACE: &str = "e2e-ui-public";

/// Logical application name used for manifest / plan identity.
pub const E2E_UI_APPLICATION: &str = "e2e-ui";

/// Explicit module identities owned by the e2e application.
///
/// These are the composition root IDs; domain crates export the typed
/// `Module` values that will be listed here once fully ported.
pub const E2E_UI_MODULE_IDS: &[&str] = &["todo", "chat", "blob", "identity"];
