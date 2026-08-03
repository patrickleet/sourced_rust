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
