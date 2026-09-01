use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "prompt", rename_all = "kebab-case")]
pub enum Prompt {
    Merge {
        path: PathBuf,
        key: String,
        repo: String,
        live: String,
    },
    MergeTarget {
        path: PathBuf,
        key: String,
        targets: Vec<String>,
        default: usize,
    },
    RemoteChanges {
        host: String,
        changes: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "choice", content = "target", rename_all = "kebab-case")]
pub enum Choice {
    Repo,
    Live,
    Ignore,
    Target(usize),
    Skip,
    Abort,
    Discard,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub prompt: Prompt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub choice: Choice,
}

#[derive(Clone)]
pub struct Client {
    requests: Sender<Request>,
    responses: Receiver<Response>,
    next_id: Arc<AtomicU64>,
}

pub struct Server {
    requests: Receiver<Request>,
    responses: Sender<Response>,
}

pub fn channel() -> (Client, Server) {
    let (request_sender, request_receiver) = crossbeam_channel::bounded(1);
    let (response_sender, response_receiver) = crossbeam_channel::bounded(1);
    (
        Client {
            requests: request_sender,
            responses: response_receiver,
            next_id: Arc::new(AtomicU64::new(1)),
        },
        Server {
            requests: request_receiver,
            responses: response_sender,
        },
    )
}

impl Client {
    pub fn choose(&self, prompt: Prompt) -> Result<Choice, String> {
        crate::cancel::check()?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.requests
            .send(Request { id, prompt })
            .map_err(|_| "the sync interface closed before a decision could be made".to_string())?;
        loop {
            crate::cancel::check()?;
            match self.responses.recv_timeout(Duration::from_millis(50)) {
                Ok(response) if response.id == id => return Ok(response.choice),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(
                        "the sync interface closed before a decision could be made".to_string()
                    );
                }
            }
        }
    }
}

impl Server {
    pub(crate) fn requests(&self) -> &Receiver<Request> {
        &self.requests
    }

    pub fn try_recv(&self) -> Option<Request> {
        self.requests.try_recv().ok()
    }

    pub fn respond(&self, request: &Request, choice: Choice) -> Result<(), String> {
        self.responses
            .send(Response {
                id: request.id,
                choice,
            })
            .map_err(|_| "the sync worker closed before receiving a decision".to_string())
    }
}

impl Prompt {
    pub fn safe_default(&self) -> Choice {
        match self {
            Self::Merge { .. } => Choice::Skip,
            Self::MergeTarget { .. } => Choice::Cancel,
            Self::RemoteChanges { .. } => Choice::Cancel,
        }
    }
}
