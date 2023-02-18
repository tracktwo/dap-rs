use std::io::{BufWriter, Write};

use serde::Serialize;
use serde_json;

use crate::{
  errors::ClientError,
  events::{Event, EventSend},
  responses::Response,
  reverse_requests::ReverseRequest,
};

pub type Result<T> = std::result::Result<T, ClientError>;

/// Client trait representing a connected DAP client that is able to receive events
/// and reverse requests.
//pub trait Client {
  /// Sends a response to the client.
//  fn respond(&mut self, response: Response) -> Result<()>;
//}

/// Trait for sending events and requests to the connected client.
pub trait Context {
  /// Sends an event to the client.
  fn send_event(&mut self, event: Event) -> Result<()>;
  /// Sends a reverse request to the client.
  fn send_reverse_request(&mut self, request: ReverseRequest) -> Result<()>;
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

  /// Get the next sequence number to use in any communication to the client.
  fn next_seq(&mut self) -> i64;

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
  Response(Response),
  Event(Event),
  ReverseRequest(ReverseRequest),
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

  fn send(&mut self, s: Sendable) -> Result<()> {
    let resp_json = serde_json::to_string(&s).map_err(ClientError::SerializationError)?;
    write!(self.stream, "Content-Length: {}\r\n\r\n", resp_json.len())
      .map_err(ClientError::IoError)?;
    log::info!("Sending json: {resp_json}");
    write!(self.stream, "{}", resp_json).map_err(ClientError::IoError)?;
    self.stream.flush()?;
    Ok(())
  }

  pub fn respond(&mut self, response: Response) -> Result<()> {
    self.send(Sendable::Response(response))
  }
}

impl<W: Write, ES: EventSend + Clone + Send + 'static> Context for BasicClient<W, ES> {
  fn send_event(&mut self, event: Event) -> Result<()> {
    self.send(Sendable::Event(event))
  }

  fn send_reverse_request(&mut self, request: ReverseRequest) -> Result<()> {
    self.send(Sendable::ReverseRequest(request))
  }

  fn request_exit(&mut self) {
    self.should_exit = true;
  }

  fn cancel_exit(&mut self) {
    self.should_exit = false;
  }

  fn get_exit_state(&self) -> bool {
    self.should_exit
  }

  fn next_seq(&mut self) -> i64 {
    self.seq_number += 1;
    self.seq_number
  }

  fn get_event_sender(&mut self) -> Box<dyn EventSend> {
    Box::new(self.event_sender.clone())
  }
}
