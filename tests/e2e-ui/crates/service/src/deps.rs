use blob_domain::BlobGame;
use chat_domain::ChatMessage;
use distributed::microsvc::RepoReadModelDependencies;
use distributed::{AggregateRepository, QueuedRepository};
use todo_domain::Todo;

pub type QueuedStore<R, L> = QueuedRepository<R, L>;
pub type TodoRepo<R, L> = AggregateRepository<QueuedStore<R, L>, Todo>;
pub type TodoDeps<R, L, S> = RepoReadModelDependencies<TodoRepo<R, L>, S>;

pub type ChatRepo<R, L> = AggregateRepository<QueuedStore<R, L>, ChatMessage>;
pub type ChatDeps<R, L, S> = RepoReadModelDependencies<ChatRepo<R, L>, S>;

pub type BlobRepo<R, L> = AggregateRepository<QueuedStore<R, L>, BlobGame>;
pub type BlobDeps<R, L, S> = RepoReadModelDependencies<BlobRepo<R, L>, S>;

/// Zitadel ingress + auth_users projector share the chat aggregate repo for outbox/leaf access
/// (ingestor is leaf-only; no chat stream is written on ingress).
pub type AuthDeps<R, L, S> = ChatDeps<R, L, S>;
