//! Provider-imported read models for e2e-ui.

pub mod auth_user_view;

pub use auth_user_view::{
    map_zitadel_user_status, map_zitadel_user_upsert, AuthUserView, ZitadelEmail,
    ZitadelUserPayload,
};
