//! Local dispatcher adapter over an in-process [`Service`].

use super::{CommandDispatchError, CommandDispatcher};
use crate::microsvc::{CommandRequest, CommandResponse, Service};
use async_trait::async_trait;
use std::sync::Arc;

/// Dispatches commands to a local executable [`Service`].
pub struct LocalCommandDispatcher {
    service: Arc<Service>,
}

impl LocalCommandDispatcher {
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }
}

#[async_trait]
impl CommandDispatcher for LocalCommandDispatcher {
    async fn dispatch(
        &self,
        request: &CommandRequest,
    ) -> Result<CommandResponse, CommandDispatchError> {
        Ok(self.service.dispatch_request(request).await)
    }

    fn kind(&self) -> &'static str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_dispatch::CommandDispatchReceipt;
    use crate::microsvc::{Routes, Service};
    use serde_json::json;
    use std::collections::HashMap;

    #[tokio::test]
    async fn local_dispatch_matches_in_process_result_and_receipt() {
        let service = Arc::new(
            Service::new().named("receipt-eq").routes(
                Routes::new().command("ping").handle(
                    |_ctx: &crate::microsvc::Context<()>| async move { Ok(json!({ "pong": true })) },
                ),
            ),
        );
        let dispatcher = LocalCommandDispatcher::new(Arc::clone(&service));
        let request = CommandRequest {
            command: "ping".into(),
            input: json!({}),
            session_variables: HashMap::new(),
        };
        let in_process = service.dispatch_request(&request).await;
        let via_dispatcher = dispatcher.dispatch(&request).await.expect("dispatch");
        assert_eq!(via_dispatcher.status, in_process.status);
        assert_eq!(via_dispatcher.body, in_process.body);
        assert_eq!(
            CommandDispatchReceipt::from_response("ping", &via_dispatcher),
            CommandDispatchReceipt::from_response("ping", &in_process)
        );
    }
}
