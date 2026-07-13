use distributed::bus::Message;
use distributed::microsvc::HandlerError;
use serde::de::DeserializeOwned;

pub fn rejected(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(e.to_string())
}

pub fn decode_payload<T: DeserializeOwned>(message: &Message) -> Result<T, HandlerError> {
    serde_json::from_slice(message.payload())
        .map_err(|e| HandlerError::Other(Box::new(e)))
}

pub fn read_model_error(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::Other(Box::new(std::io::Error::other(e.to_string())))
}
