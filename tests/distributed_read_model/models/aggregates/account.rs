use serde::{Deserialize, Serialize};
use sourced_rust::{sourced, Entity, Snapshot};

#[derive(Default, Snapshot)]
pub struct Account {
    pub entity: Entity,
    pub owner: String,
    pub balance_cents: i64,
    pub is_open: bool,
}

#[sourced(entity)]
impl Account {
    #[event("AccountOpened")]
    pub fn open(&mut self, id: String, owner: String) {
        self.entity.set_id(&id);
        self.owner = owner;
        self.balance_cents = 0;
        self.is_open = true;
    }

    #[event("MoneyDeposited", when = self.is_open && amount_cents > 0)]
    pub fn deposit(&mut self, amount_cents: i64) {
        self.balance_cents += amount_cents;
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenAccount {
    pub id: String,
    pub owner: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DepositMoney {
    pub id: String,
    pub amount_cents: i64,
}
