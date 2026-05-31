//! Read-only query service for the board read model. Primary-key loads plus
//! `has_many` / `belongs_to` relationship includes.

use distributed::{AsyncReadModelWorkspaceExt, InMemoryReadModelStore, ReadModelError};

use crate::read_models::{board_key, card_key, BoardView, CardView};

#[derive(Clone)]
pub struct BoardQueryService {
    store: InMemoryReadModelStore,
}

impl BoardQueryService {
    pub fn new(store: InMemoryReadModelStore) -> Self {
        Self { store }
    }

    /// Load a board with its cards (`has_many` include).
    pub async fn board_with_cards(
        &self,
        board_id: &str,
    ) -> Result<Option<BoardView>, ReadModelError> {
        let mut session = self.store.workspace_async();
        Ok(session
            .load_async::<BoardView>(board_key(board_id))
            .include("cards")
            .one()
            .await?
            .map(|view| view.data))
    }

    /// Load one card with its board (`belongs_to` include).
    pub async fn card_with_board(
        &self,
        board_id: &str,
        card_id: &str,
    ) -> Result<Option<CardView>, ReadModelError> {
        let mut session = self.store.workspace_async();
        Ok(session
            .load_async::<CardView>(card_key(board_id, card_id))
            .include("board")
            .one()
            .await?
            .map(|view| view.data))
    }
}
