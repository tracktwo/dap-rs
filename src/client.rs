use std::{io::{BufWriter, Write}, sync::mpsc::SendError};

use serde::Serialize;
use serde_json;

use crate::{
  errors::ClientError,
  events::{Event, EventProtocolMessage, EventSend},
  responses::{Response, ResponseProtocolMessage},
  reverse_requests::{ReverseRequest, ReverseRequestProtocolMessage},
};

/// Trait for sending events and requests to the connected client.
pub trait Context {
  /// Sends an event to the client.
  fn send_event(&mut self, event: Event) -> Result<(), SendError<Event>>;

  // TODO: Reimplement reverse requests.
  /// Sends a reverse request to the client.
  //fn send_reverse_request(&mut self, request: ReverseRequest) -> Result<()>;
  /// Notifies the server that it should gracefully exit after `accept`
  /// returned.
  ///
  /// It is recommended to send a `Terminated` and/or `Stopped` event to the client.
  fn request_exit(&mut self);
  /// Clears an exit request set by `request_exit` in the same `accept` call.
  /// This cannot be used to clear an exit request that happened during a previous
  /// `accept`.
  fn cancel_exit(&mut self);
  /// Returns `true` if the exiting was requested.
  fn get_exit_state(&self) -> bool;

  /// Get an object that can be used to send events to the client at any time.
  fn get_event_sender(&mut self) -> Box<dyn EventSend>;
}

pub struct BasicClient<W: Write, ES: EventSend + Clone + Send + 'static> {
  stream: BufWriter<W>,
  should_exit: bool,
  seq_number: i64,
  event_sender: ES,
}

#[derive(Serialize, Debug)]
#[serde(untagged)]
enum Sendable {
  Response(ResponseProtocolMessage),
  Event(EventProtocolMessage),
  ReverseRequest(ReverseRequestProtocolMessage),
}

impl<W: Write, ES: EventSend + Clone + Send> BasicClient<W, ES> {
  pub fn new(stream: W, ev: ES) -> Self {
    Self {
      stream: BufWriter::new(stream),
      should_exit: false,
      event_sender: ev,
      seq_number: 0,
    }
  }

  /// Writes a DAP message to the client stream.
  ///
  /// This is json-encoded and prefixed by a content-length header.
  fn send(&mut self, s: Sendable) -> Result<(), ClientError> {
    let resp_json = serde_json::to_string(&s).map_err(ClientError::SerializationError)?;
    log::info!("Sending length: {}", resp_json.len());
    write!(self.stream, "Content-Length: {}\r\n\r\n", resp_json.len())
      .map_err(ClientError::IoError)?;
    log::info!("Sending json: {resp_json}");
    write!(self.stream, "{}", resp_json).map_err(ClientError::IoError)?;
    self.stream.flush()?;
    Ok(())
  }

  /// Dispatch a response to the DAP client.
  pub fn respond(&mut self, response: Response) -> Result<(), ClientError> {
    let seq = self.next_seq();
    self.send(Sendable::Response(ResponseProtocolMessage {
      seq,
      response,
    }))
  }

  /// Dispatch an event to the DAP client.
  pub fn send_event_impl(&mut self, event: Event) -> Result<(), ClientError> {
    let seq = self.next_seq();
    self.send(Sendable::Event(EventProtocolMessage { seq, event }))
  }

  /// Dispatch a reverse request to the DAP client.
  pub fn send_reverse_request(&mut self, req: ReverseRequest) -> Result<(), ClientError> {
    let seq = self.next_seq();
    self.send(Sendable::ReverseRequest(ReverseRequestProtocolMessage {
      seq,
      req,
    }))
  }

  /// Return the next sequence number to use, incrementing the state.
  fn next_seq(&mut self) -> i64 {
    self.seq_number += 1;
    self.seq_number
  }
}

impl<W: Write, ES: EventSend + Clone + Send + 'static> Context for BasicClient<W, ES> {

  fn request_exit(&mut self) {
    self.should_exit = true;
  }

  fn cancel_exit(&mut self) {
    self.should_exit = false;
  }

  fn get_exit_state(&self) -> bool {
    self.should_exit
  }

  fn get_event_sender(&mut self) -> Box<dyn EventSend> {
    Box::new(self.event_sender.clone())
  }

  fn send_event(&mut self, event: Event) -> Result<(), SendError<Event>> {
    self.event_sender.send_event(event)
  }
}
