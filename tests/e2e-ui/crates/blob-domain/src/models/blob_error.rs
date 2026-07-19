use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlobError {
    #[error("game already exists")]
    AlreadyExists,
    #[error("game not found / not created")]
    NotCreated,
    #[error("empty game id")]
    EmptyId,
    #[error("empty owner id")]
    EmptyOwner,
    #[error("not the owner")]
    NotOwner,
    #[error("player is dead")]
    PlayerDead,
    #[error("current level not completed")]
    LevelNotComplete,
    #[error("no active level")]
    NoActiveLevel,
    #[error("invalid map: {0}")]
    InvalidMap(String),
    #[error("cannot move: {0}")]
    CannotMove(String),
    #[error(transparent)]
    Event(#[from] distributed::EventRecordError),
}
