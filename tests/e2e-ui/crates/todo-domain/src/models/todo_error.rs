use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TodoError {
    #[error("todo already exists")]
    AlreadyExists,
    #[error("todo not found / not created")]
    NotCreated,
    #[error("todo is archived")]
    Archived,
    #[error("todo is already completed")]
    AlreadyCompleted,
    #[error("todo is not completed")]
    NotCompleted,
    #[error("empty todo id")]
    EmptyId,
    #[error("empty owner id")]
    EmptyOwner,
    #[error("empty title")]
    EmptyTitle,
    #[error("not the owner")]
    NotOwner,
    #[error(transparent)]
    Event(#[from] distributed::EventRecordError),
}
