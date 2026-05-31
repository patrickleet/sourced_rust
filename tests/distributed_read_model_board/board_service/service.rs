use std::sync::Arc;

use distributed::microsvc::Service;

use super::{handlers, BoardRepo};

pub fn model_service(repo: BoardRepo) -> Arc<Service<BoardRepo>> {
    Arc::new(distributed::register_handlers!(
        Service::with_repo(repo),
        command handlers::board_open,
        command handlers::board_add_card,
        command handlers::board_move_card,
        command handlers::board_remove_card,
    ))
}
