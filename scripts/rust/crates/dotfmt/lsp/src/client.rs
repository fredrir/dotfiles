//! Requests initiated by the server, routed back through the main loop.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use lsp_server::{Message, Request, RequestId, Response};
use serde::Serialize;
use serde_json::Value;

pub struct Client {
    sender: crossbeam_channel::Sender<Message>,
    next: AtomicI32,
    pending: Mutex<HashMap<RequestId, crossbeam_channel::Sender<Result<Value, String>>>>,
    closed: AtomicBool,
}

impl Client {
    pub fn new(sender: crossbeam_channel::Sender<Message>) -> Self {
        Self {
            sender,
            next: AtomicI32::new(1),
            pending: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub fn sender(&self) -> crossbeam_channel::Sender<Message> {
        self.sender.clone()
    }

    pub fn request(&self, method: &str, params: impl Serialize) -> Result<Value, String> {
        let params = serde_json::to_value(params).map_err(|error| error.to_string())?;
        let id = RequestId::from(self.next.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = crossbeam_channel::bounded(1);
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if self.closed.load(Ordering::SeqCst) {
                return Err("client is shut down".into());
            }
            pending.insert(id.clone(), sender);
        }
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

    /// Rejects every pending and future server-initiated request.
    ///
    /// Called when the main loop exits: a configuration pull must not keep the
    /// writer thread alive for its 30-second timeout after `exit`.
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for (_, sender) in pending {
            let _ = sender.send(Err("client is shut down".into()));
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
        if self.closed.load(Ordering::SeqCst) {
            return Err("client is shut down".into());
        }
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


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn shutdown_wakes_pending_requests_and_rejects_new_ones() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let client = Arc::new(Client::new(sender));
        let waiting = {
            let client = Arc::clone(&client);
            thread::spawn(move || client.request("workspace/configuration", json!({})))
        };
        let _request = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("request was sent");
        client.shutdown();
        let result = waiting.join().expect("request thread");
        assert!(result.is_err(), "pending request should be rejected");
        assert!(client.request("workspace/configuration", json!({})).is_err());
    }
}
