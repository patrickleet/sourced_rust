use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use crate::graphql::SurfaceProjector;
use crate::projection_protocol::{CompiledProjectionTopology, ProjectionEpoch};
use crate::read_model::RelationalReadModel;
use crate::table::TableSchema;

use super::super::dependencies::CausalProjectionRouteDependencies;
use super::super::service::{HandlerSpec, Routes};
use super::super::HandlerError;
use super::context::CausalProjectorContext;
use super::runtime::RegisteredProjector;

type ProjectorHandlerFuture =
    Pin<Box<dyn Future<Output = Result<(), HandlerError>> + Send + 'static>>;
pub(super) type ProjectorHandlerFn<I> =
    dyn Fn(CausalProjectorContext, I) -> ProjectorHandlerFuture + Send + Sync;
fn boxed_projector_handler<I, F, Fut>(handler: F) -> Arc<ProjectorHandlerFn<I>>
where
    I: Send + 'static,
    F: Fn(CausalProjectorContext, I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
{
    Arc::new(move |context, input| Box::pin(handler(context, input)))
}

/// Builder for one typed causal projector route.
pub struct CausalProjectorRouteBuilder<D, I> {
    routes: Routes<D>,
    declaration: SurfaceProjector,
    schemas: Vec<&'static TableSchema>,
    _input: PhantomData<fn(I)>,
}

impl<D, I> CausalProjectorRouteBuilder<D, I>
where
    D: CausalProjectionRouteDependencies + Send + Sync + 'static,
    I: serde::de::DeserializeOwned + Send + 'static,
{
    pub(in crate::microsvc) fn new(routes: Routes<D>, declaration: SurfaceProjector) -> Self {
        Self {
            routes,
            declaration,
            schemas: Vec::new(),
            _input: PhantomData,
        }
    }

    /// Register one complete typed output model. Call once for every model in
    /// the reused [`SurfaceProjector`] declaration.
    pub fn model<M>(mut self) -> Self
    where
        M: RelationalReadModel + 'static,
    {
        self.schemas.push(M::schema());
        self
    }

    /// Register the capability-restricted handler.
    ///
    /// Invalid/incomplete topology declarations panic at service construction,
    /// before transport traffic can start, matching ordinary duplicate-route
    /// registration behavior.
    pub fn handle<F, Fut>(self, handler: F) -> Routes<D>
    where
        F: Fn(CausalProjectorContext, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
    {
        if self.declaration.facts.is_empty() {
            panic!(
                "causal projector `{}` requires at least one accepted fact",
                self.declaration.name
            );
        }
        let change_epoch = self.declaration.change_epoch.as_ref().unwrap_or_else(|| {
            panic!(
                "causal projector `{}` requires a change-log epoch",
                self.declaration.name
            )
        });
        let change_epoch = ProjectionEpoch::new(change_epoch.clone()).unwrap_or_else(|error| {
            panic!(
                "causal projector `{}` has an invalid change-log epoch: {error}",
                self.declaration.name
            )
        });
        let compiled = CompiledProjectionTopology::compile(
            &self.declaration.name,
            &self.declaration.facts,
            &self.declaration.models,
            &self.declaration.partition,
            self.schemas,
        )
        .unwrap_or_else(|error| {
            panic!(
                "causal projector `{}` has an invalid compiled topology: {error}",
                self.declaration.name
            )
        });
        let spec = HandlerSpec::projector(self.declaration.facts.clone());
        self.routes.register_projector(
            spec,
            Box::new(RegisteredProjector::<I> {
                compiled,
                change_epoch,
                handle: boxed_projector_handler(handler),
            }),
        )
    }
}
