//! Requests initiated by the server, routed back through the main loop.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use lsp_server::{Message, Request, RequestId, Response};
use serde::Serialize;
use serde_json::Value;

pub struct Client {
    sender: crossbeam_channel::Sender<Message>,
    next: AtomicI32,
    pending: Mutex<HashMap<RequestId, crossbeam_channel::Sender<Result<Value, String>>>>,
}

impl Client {
    pub fn new(sender: crossbeam_channel::Sender<Message>) -> Self {
        Self {
            sender,
            next: AtomicI32::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn sender(&self) -> crossbeam_channel::Sender<Message> {
        self.sender.clone()
    }

    pub fn request(&self, method: &str, params: impl Serialize) -> Result<Value, String> {
        let params = serde_json::to_value(params).map_err(|error| error.to_string())?;
        let id = RequestId::from(self.next.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = crossbeam_channel::bounded(1);
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.clone(), sender);
        self.sender
            .send(Message::Request(Request {
                id: id.clone(),
                method: method.into(),
                params,
            }))
            .map_err(|error| error.to_string())?;
        match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                self.forget(&id);
                Err(error)
            }
            Err(_) => {
                self.forget(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    fn forget(&self, id: &RequestId) {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
    }

    /// Sends a server-initiated request without waiting for its response.
    /// Used for notifications that are requests in the protocol, such as
    /// `client/registerCapability`.
    pub fn send_request(&self, method: &str, params: impl Serialize) -> Result<RequestId, String> {
        let params = serde_json::to_value(params).map_err(|error| error.to_string())?;
        let id = RequestId::from(self.next.fetch_add(1, Ordering::Relaxed));
        self.sender
            .send(Message::Request(Request {
                id: id.clone(),
                method: method.into(),
                params,
            }))
            .map_err(|error| error.to_string())?;
        Ok(id)
    }

    /// Completes a request previously sent through [`Client::request`].
    pub fn response(&self, response: Response) {
        let Some(sender) = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&response.id)
        else {
            return;
        };
        let _ = sender.send(match response.response_result {
            Ok(value) => Ok(value),
            Err(error) => Err(error.message),
        });
    }
}
