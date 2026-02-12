//! Saga Pattern Tests
//!
//! This module demonstrates the saga pattern using event sourcing.
//! A saga is an event-sourced aggregate that coordinates a multi-step
//! business process across multiple aggregates, with compensation
//! (rollback) capabilities when steps fail.

mod handlers;
mod order;
mod orchestration;
mod distributed;
mod microsvc_saga;
