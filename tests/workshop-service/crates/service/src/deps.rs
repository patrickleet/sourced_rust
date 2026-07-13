use distributed::microsvc::RepoReadModelDependencies;
use distributed::{AggregateRepository, QueuedRepository};
use workshop_catalog_domain::Product;
use workshop_orders_domain::WorkshopOrder;

pub type QueuedStore<R, L> = QueuedRepository<R, L>;
pub type ProductRepo<R, L> = AggregateRepository<QueuedStore<R, L>, Product>;
pub type OrderRepo<R, L> = AggregateRepository<QueuedStore<R, L>, WorkshopOrder>;

pub type ProductDeps<R, L, S> = RepoReadModelDependencies<ProductRepo<R, L>, S>;
pub type OrderDeps<R, L, S> = RepoReadModelDependencies<OrderRepo<R, L>, S>;
