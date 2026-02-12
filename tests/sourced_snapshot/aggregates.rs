use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity, Snapshot};

// ============================================================================
// Default case: id + all fields
// ============================================================================

#[derive(Default, Snapshot)]
pub struct Todo {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

impl Todo {
    pub fn new() -> Self {
        Self::default()
    }

    #[digest("Initialized")]
    pub fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[digest("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

sourced_rust::aggregate!(Todo, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});

// ============================================================================
// Custom ID key: snapshot(id = "sku")
// ============================================================================

#[derive(Default, Snapshot)]
#[snapshot(id = "sku")]
pub struct Inventory {
    pub entity: Entity,
    pub sku: String,
    pub available: u32,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    #[digest("Created")]
    pub fn create(&mut self, id: String, sku: String, available: u32) {
        self.entity.set_id(&id);
        self.sku = sku;
        self.available = available;
    }

    #[digest("Restocked")]
    pub fn restock(&mut self, qty: u32) {
        self.available += qty;
    }
}

sourced_rust::aggregate!(Inventory, entity {
    "Created"(id, sku, available) => create,
    "Restocked"(qty) => restock,
});

// ============================================================================
// serde(skip) field exclusion
// ============================================================================

#[derive(Default, Serialize, Deserialize, Snapshot)]
pub struct Order {
    pub entity: Entity,
    pub customer: String,
    pub total: u64,
    #[serde(skip)]
    pub cached_label: String,
}

impl Order {
    pub fn new() -> Self {
        Self::default()
    }

    #[digest("Placed")]
    pub fn place(&mut self, id: String, customer: String, total: u64) {
        self.entity.set_id(&id);
        self.customer = customer;
        self.total = total;
        self.cached_label = format!("Order for {}", self.customer);
    }
}

sourced_rust::aggregate!(Order, entity {
    "Placed"(id, customer, total) => place,
});

// ============================================================================
// Works with #[sourced(entity)] on impl
// ============================================================================

#[derive(Default, Snapshot)]
pub struct Counter {
    pub entity: Entity,
    pub count: i64,
}

#[sourced_rust::sourced(entity)]
impl Counter {
    pub fn new() -> Self {
        Self::default()
    }

    #[event("Initialized")]
    pub fn initialize(&mut self, id: String) {
        self.entity.set_id(&id);
    }

    #[event("Incremented")]
    pub fn increment(&mut self, amount: i64) {
        self.count += amount;
    }
}

// ============================================================================
// Custom entity field name
// ============================================================================

#[derive(Default, Snapshot)]
#[snapshot(entity = "my_entity")]
pub struct Widget {
    pub my_entity: Entity,
    pub name: String,
    pub weight: f64,
}

impl Widget {
    pub fn new() -> Self {
        Self::default()
    }

    #[digest(my_entity, "Created")]
    pub fn create(&mut self, id: String, name: String, weight: f64) {
        self.my_entity.set_id(&id);
        self.name = name;
        self.weight = weight;
    }
}

sourced_rust::aggregate!(Widget, my_entity {
    "Created"(id, name, weight) => create,
});

// ============================================================================
// serde(skip, default) field exclusion (EntityEmitter pattern)
// ============================================================================

#[derive(Default, Serialize, Deserialize)]
pub struct DummyEmitter;

#[allow(dead_code)]
#[derive(Default, Serialize, Deserialize, Snapshot)]
pub struct Notifier {
    pub entity: Entity,
    pub message: String,
    #[serde(skip, default)]
    pub emitter: DummyEmitter,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    #[digest("Sent")]
    pub fn send(&mut self, id: String, message: String) {
        self.entity.set_id(&id);
        self.message = message;
    }
}

sourced_rust::aggregate!(Notifier, entity {
    "Sent"(id, message) => send,
});
