use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct EventEmitter {
    listeners: Arc<RwLock<HashMap<String, Vec<Box<dyn Fn(&dyn Any) + Send + Sync>>>>>,
}

impl EventEmitter {
    pub fn new() -> Self {
        EventEmitter {
            listeners: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn on<F>(&self, event: String, listener: F)
    where
        F: Fn(&dyn Any) + Send + Sync + 'static,
    {
        let mut listeners = self.listeners.write().unwrap();
        listeners
            .entry(event)
            .or_insert_with(Vec::new)
            .push(Box::new(listener));
    }

    pub fn emit(&self, event: &str, data: &dyn Any) {
        let listeners = self.listeners.read().unwrap();
        if let Some(event_listeners) = listeners.get(event) {
            for listener in event_listeners {
                listener(data);
            }
        }
    }

    pub fn remove_listener<F>(&self, event: &str, listener_to_remove: F)
    where
        F: Fn(&dyn Any) + Send + Sync + 'static,
    {
        let mut listeners = self.listeners.write().unwrap();
        if let Some(event_listeners) = listeners.get_mut(event) {
            event_listeners.retain(|listener| {
                // Use std::ptr::addr_eq to compare the function pointers
                !std::ptr::addr_eq(listener.as_ref(), &listener_to_remove)
            });
        }
    }
}

impl Default for EventEmitter {
    fn default() -> Self {
        Self::new()
    }
}
