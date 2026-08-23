use distributed::microsvc::RepoReadModelDependencies;
use distributed::{AggregateRepository, QueuedRepository};

use crate::aggregate::Identity;

pub type QueuedStore<R, L> = QueuedRepository<R, L>;

pub type IdentityRepo<R, L> = AggregateRepository<QueuedStore<R, L>, Identity>;
pub type AuthDeps<R, L, S> = RepoReadModelDependencies<IdentityRepo<R, L>, S>;
