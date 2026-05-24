use serde::{Deserialize, Serialize};
use sourced_rust::{sourced, Entity, Snapshot};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CardState {
    pub card_id: String,
    pub column: String,
    pub title: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
}

#[derive(Default, Snapshot)]
pub struct Board {
    pub entity: Entity,
    pub name: String,
    pub cards: Vec<CardState>,
}

#[sourced(entity)]
impl Board {
    #[event("BoardOpened")]
    pub fn open(&mut self, id: String, name: String) {
        self.entity.set_id(&id);
        self.name = name;
    }

    #[event("CardAdded")]
    pub fn add_card(
        &mut self,
        card_id: String,
        column: String,
        title: String,
        labels: Vec<String>,
        assignee: Option<String>,
    ) {
        if !self.cards.iter().any(|card| card.card_id == card_id) {
            self.cards.push(CardState {
                card_id,
                column,
                title,
                labels,
                assignee,
            });
        }
    }

    #[event("CardMoved")]
    pub fn move_card(&mut self, card_id: String, column: String) {
        if let Some(card) = self.cards.iter_mut().find(|card| card.card_id == card_id) {
            card.column = column;
        }
    }

    #[event("CardRemoved")]
    pub fn remove_card(&mut self, card_id: String) {
        self.cards.retain(|card| card.card_id != card_id);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenBoard {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddCard {
    pub id: String,
    pub card_id: String,
    pub column: String,
    pub title: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoveCard {
    pub id: String,
    pub card_id: String,
    pub column: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoveCard {
    pub id: String,
    pub card_id: String,
}
