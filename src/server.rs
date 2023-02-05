use std::fmt::Debug;
use std::io::BufRead;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::SendError;
use std::sync::mpsc::Sender;

use serde_json;

use crate::adapter::Adapter;
use crate::client::Client;
use crate::client::Context;
use crate::errors::{DeserializationError, ServerError};
use crate::events::EventBody;
use crate::prelude::ResponseBody;
use crate::requests::Request;

/// Message types for the server message processing queue.
///
/// The server processes messages of this type from a queue and dispatches
/// information to the client.
enum ServerMessage<A: Adapter>
where
  <A as Adapter>::Error: Debug + Sized,
{
  /// An event to send to the client.
  ///
  /// Events are controlled by the adapter and can be send to the client at
  /// any time. These are sent as 'event' messages to the client.
  Event(EventBody),

  /// A request from the client.
  ///
  /// Client requests are managed internally by the server. Request messages
  /// will be published to the adapter for processing and will receive back
  /// a response to be sent to the client.
  Request(Request),

  /// A message indicating an error condition from the DAP message processing
  /// loop.
  ServerError(ServerError<A::Error>),

  /// A message indicating the server has stopped and the message queue should
  /// now also stop.
  Quit,
}

/// A type-safe wrapper for the server's message queue channel sender that
/// only accepts events.
///
/// Ensures the adapter cannot send 'Request' messages through the channel,
/// only events.
pub struct EventSender<A: Adapter>
where
  <A as Adapter>::Error: Debug + Sized,
{
  sender: Sender<ServerMessage<A>>,
}

impl<A: Adapter> EventSender<A>
where
  <A as Adapter>::Error: Debug + Sized,
{
  /// Send an event body through the sender channel.
  pub fn send(&self, t: EventBody) -> Result<(), SendError<EventBody>> {
    self
      .sender
      .send(ServerMessage::Event(t))
      .or_else(|err| match err.0 {
        ServerMessage::Event(body) => Err(SendError(body)),
        _ => panic!("Received a request error from an event send?!"),
      })
  }
}

/// Ties together an Adapter and a Client.
///
/// The `Server` is responsible for reading the incoming bytestream and constructing deserialized
/// requests from it; calling the `accept` function of the `Adapter` and passing the response
/// to the client.
pub struct Server<A: Adapter, C: Client + Context>
where
  <A as Adapter>::Error: Debug + Sized,
{
  adapter: A,
  client: C,
  sender: Sender<ServerMessage<A>>,
  receiver: Receiver<ServerMessage<A>>,
}

impl<A: Adapter + 'static, C: Client + Context> Server<A, C>
where
  <A as Adapter>::Error: Debug + Sized + Send,
{
  /// Construct a new Server and take ownership of the adapter and client.
  pub fn new(adapter: A, client: C) -> Self {
    let (sender, receiver) = mpsc::channel();
    Self {
      adapter,
      client,
      sender,
      receiver,
    }
  }

  // Clone a sender for the server's message channel.
  pub fn clone_sender(&self) -> EventSender<A> {
    let dupe = self.sender.clone();
    EventSender { sender: dupe }
  }

  /// Run the server.
  ///
  /// This will start reading the `input` buffer that is passed to it and will try to interpert
  /// the incoming bytes according to the DAP protocol.
  pub fn run<Buf: BufRead + Send + 'static>(
    &mut self,
    input: Buf,
  ) -> Result<(), ServerError<A::Error>> {
    let sender = self.sender.clone();
    let message_thread = std::thread::spawn(move || {
      // Keep a clone of the message queue sender handy: if the main message processing
      // loop exits with an error then we need to dispatch a final message to the queue
      // so the 'run' loop knows it is time to exit.
      let emerg_sender = sender.clone();
      if let Err(e) = Self::process_messages(input, sender) {
        // Message queue has failed with an error. Push this error into the queue and return.
        // If this message send fails then we're truly in trouble as we have stopped receiving
        // messages but have no way to tell the message loop to stop, so just panic.
        emerg_sender.send(ServerMessage::ServerError(e)).unwrap();
      }
      ()
    });

    let mut err: Result<(), ServerError<A::Error>> = Ok(());

    // Read messages from the message queue, until the main message processing thread
    // tells us to stop.
    for msg in self.receiver.iter() {
      match msg {
        ServerMessage::Request(req) => match self.adapter.accept(req, &mut self.client) {
          Ok(response) => match response.body {
            Some(ResponseBody::Empty) => (),
            _ => {
              self
                .client
                .respond(response)
                .map_err(ServerError::ClientError)?;
            }
          },
          Err(e) => return Err(ServerError::AdapterError(e)),
        },

        ServerMessage::Event(_body) => {
          // TODO Handle events
        }

        ServerMessage::ServerError(e) => {
          err = Err(e);
          break;
        }

        ServerMessage::Quit => break,
      };
    }

    message_thread
      .join()
      .or_else(|_| Err(ServerError::IoError))?;

    err
  }

  /// Main message processing loop. Reads requests continuously from the buffer provided,
  /// parses them into DAP requests, and dispatches them to the adapter message queue.
  fn process_messages<Buf: BufRead>(
    mut input: Buf,
    sender: Sender<ServerMessage<A>>,
  ) -> Result<(), ServerError<A::Error>>
  where
    <A as Adapter>::Error: Debug + Sized,
  {
    let mut buffer = String::new();
    loop {
      let content_length: usize;
      buffer.clear();

      // Parse the header. The current DAP spec only supports a single header:
      // Content-Length.
      //
      // If the client has closed the connection then we are done
      // and can put a graceful exit message in the queue before returning.
      //
      // We expect the client to only close the connection between requests, not
      // in the middle of one, so the other cases here consider an EOF to be an
      // error.
      match input.read_line(&mut buffer).or(Err(ServerError::IoError)) {
        Ok(0) => {
          sender
            .send(ServerMessage::Quit)
            .or(Err(ServerError::IoError))?;
          return Ok(());
        }
        Err(e) => return Err(e),
        _ => (),
      };

      let parts: Vec<&str> = buffer.trim_end().split(':').collect();
      if parts.len() == 2 {
        match parts[0] {
          "Content-Length" => {
            content_length = match parts[1].trim().parse() {
              Ok(val) => val,
              Err(_) => return Err(ServerError::HeaderParseError { line: buffer }),
            };
          }
          other => {
            return Err(ServerError::UnknownHeader {
              header: other.to_string(),
            })
          }
        }
      } else {
        return Err(ServerError::HeaderParseError { line: buffer });
      }

      // The content-length header should be followed by one empty line.
      buffer.clear();
      input.read_line(&mut buffer).or(Err(ServerError::IoError))?;
      if buffer != "\r\n" {
        return Err(ServerError::HeaderParseError { line: buffer });
      }

      // Now parse the content. We cannot use read_line here as we are expecting
      // the JSON payload to have exactly content_length bytes and it is not
      // necessarily terminated by a \r\n pair (e.g. VSCode does not put linefeeds
      // on the end of its requests).
      let mut buffer = Vec::with_capacity(content_length);
      buffer.resize(content_length, 0);
      input
        .read_exact(&mut buffer)
        .or(Err(ServerError::IoError))?;

      let request: Request = match serde_json::from_slice(&buffer) {
        Ok(val) => val,
        Err(e) => return Err(ServerError::ParseError(DeserializationError::SerdeError(e))),
      };

      // Send the request to the queue for processing.
      sender
        .send(ServerMessage::Request(request))
        .or(Err(ServerError::IoError))?;
    }
  }
}
