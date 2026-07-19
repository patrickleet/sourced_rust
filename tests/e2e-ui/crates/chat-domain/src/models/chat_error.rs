use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChatError {
    #[error("message already exists")]
    AlreadyExists,
    #[error("empty message id")]
    EmptyId,
    #[error("empty author id")]
    EmptyAuthor,
    #[error("empty room id")]
    EmptyRoom,
    #[error("empty body")]
    EmptyBody,
    #[error(transparent)]
    Event(#[from] distributed::EventRecordError),
}
