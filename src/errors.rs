use thiserror::Error;
use std::fmt::Debug;

#[derive(Debug, Error)]
pub enum DeserializationError {
  #[error("could not parse value '{value}' to enum variant of '{enum_name}'")]
  StringToEnumParseError { enum_name: String, value: String },
  #[error("Error while deserializing: {0}")]
  SerdeError(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum ServerError<AE: Debug> {
  #[error("I/O error")]
  IoError,

  #[error("Unknown header: {header}")]
  UnknownHeader { header: String },

  #[error("Parse error: {0}")]
  ParseError(#[from] DeserializationError),

  #[error("Could not parse header line '{line}'")]
  HeaderParseError { line: String },

  #[error("Protocol error while reading line '{line}', reason: '{reason}'")]
  ProtocolError { reason: String, line: String },

  #[error("Error while sending to client")]
  ClientError(#[from] ClientError),

  #[error(transparent)]
  AdapterError(AE),
}

#[derive(Debug, Error)]
pub enum AdapterError {
  #[error("Trying to contruct a non-sense response (such as an ACK for a request that requires a response body")]
  ResponseContructError,
}

#[derive(Debug, Error)]
pub enum ClientError {
  #[error("I/O error")]
  IoError(#[from] std::io::Error),

  #[error("Serialization error")]
  SerializationError(#[from] serde_json::Error),
}
