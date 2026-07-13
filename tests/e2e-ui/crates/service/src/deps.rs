use chat_domain::ChatMessage;
use distributed::microsvc::RepoReadModelDependencies;
use distributed::{AggregateRepository, QueuedRepository};
use todo_domain::Todo;

pub type QueuedStore<R, L> = QueuedRepository<R, L>;
pub type TodoRepo<R, L> = AggregateRepository<QueuedStore<R, L>, Todo>;
pub type TodoDeps<R, L, S> = RepoReadModelDependencies<TodoRepo<R, L>, S>;

pub type ChatRepo<R, L> = AggregateRepository<QueuedStore<R, L>, ChatMessage>;
pub type ChatDeps<R, L, S> = RepoReadModelDependencies<ChatRepo<R, L>, S>;
