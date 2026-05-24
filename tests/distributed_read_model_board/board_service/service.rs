use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, BoardRepo};

pub fn model_service(repo: BoardRepo) -> Arc<Service<BoardRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::board_open,
        handlers::board_add_card,
        handlers::board_move_card,
        handlers::board_remove_card,
    ))
}
