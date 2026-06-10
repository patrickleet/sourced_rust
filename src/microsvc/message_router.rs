//! `microsvc::Service<D>` as a transport [`MessageRouter`].
//!
//! This is the bridge that lets the bus consume side run a service without the
//! bus naming `Service`. Handler-error classification into the transport's
//! retryable/permanent vocabulary happens here, on the microsvc side, so the
//! bus-core runner only ever sees an already-classified `TransportError`.

use crate::bus::{MessageRouter, TransportError};
use crate::microsvc::{Message, MessageKind, Service, SubscriptionPlan};

impl<D: Send + Sync + 'static> MessageRouter for Service<D> {
    fn consumer_group(&self) -> Option<&str> {
        self.name()
    }

    fn handles(&self, kind: MessageKind, name: &str) -> bool {
        self.handles_message(kind, name)
    }

    fn subscription_plan(&self) -> SubscriptionPlan {
        // Call the inherent method, not this trait method (which would recurse).
        Service::subscription_plan(self)
    }

    async fn dispatch(&self, message: &Message) -> Result<(), TransportError> {
        self.dispatch_message(message)
            .await
            .map(|_| ())
            .map_err(TransportError::from)
    }
}
