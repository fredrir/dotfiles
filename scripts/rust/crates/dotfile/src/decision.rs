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
        dotfiles: String,
        local: String,
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
    Overwrite {
        subject: Subject,
        path: PathBuf,
        detail: String,
        repo: Option<String>,
        live: Option<String>,
        index: usize,
        total: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Subject {
    UnmanagedPath,
    Secret,
    Tool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "choice", content = "target", rename_all = "kebab-case")]
pub enum Choice {
    Save,
    Ignore,
    Target(usize),
    Discard,
    Cancel,
    Overwrite,
    Keep,
    OverwriteAll,
    KeepAll,
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
    promptable: bool,
}

pub struct Server {
    requests: Receiver<Request>,
    responses: Sender<Response>,
}

pub fn channel() -> (Client, Server) {
    channel_for(true)
}

/// `promptable` is false when no interface can ask a question, only answer it safely.
pub fn channel_for(promptable: bool) -> (Client, Server) {
    let (request_sender, request_receiver) = crossbeam_channel::bounded(1);
    let (response_sender, response_receiver) = crossbeam_channel::bounded(1);
    (
        Client {
            requests: request_sender,
            responses: response_receiver,
            next_id: Arc::new(AtomicU64::new(1)),
            promptable,
        },
        Server {
            requests: request_receiver,
            responses: response_sender,
        },
    )
}

impl Client {
    pub fn promptable(&self) -> bool {
        self.promptable
    }

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

    pub fn next(&self) -> Option<Request> {
        self.requests.recv().ok()
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
    /// The answer used when nobody can be asked.
    pub fn safe_default(&self) -> Choice {
        match self {
            Self::Merge { .. } => Choice::Ignore,
            Self::MergeTarget { .. } => Choice::Cancel,
            Self::RemoteChanges { .. } => Choice::Cancel,
            Self::Overwrite { .. } => Choice::Keep,
        }
    }

    pub fn preselected(&self) -> Choice {
        match self {
            Self::Overwrite { .. } => Choice::Overwrite,
            prompt => prompt.safe_default(),
        }
    }

    pub fn cancellation(&self) -> Choice {
        match self {
            Self::Merge { .. } => Choice::Ignore,
            Self::MergeTarget { .. } | Self::RemoteChanges { .. } => Choice::Cancel,
            Self::Overwrite { .. } => Choice::Keep,
        }
    }

    pub fn accepts(&self, choice: Choice) -> bool {
        match self {
            Self::Merge { .. } => matches!(choice, Choice::Save | Choice::Ignore),
            Self::MergeTarget { targets, .. } => match choice {
                Choice::Target(index) => index < targets.len(),
                Choice::Cancel => true,
                _ => false,
            },
            Self::RemoteChanges { .. } => matches!(choice, Choice::Discard | Choice::Cancel),
            Self::Overwrite { .. } => matches!(
                choice,
                Choice::Overwrite | Choice::Keep | Choice::OverwriteAll | Choice::KeepAll
            ),
        }
    }

    /// Answers that settle every remaining prompt of the same subject.
    pub fn batched(choice: Choice) -> Option<bool> {
        match choice {
            Choice::OverwriteAll => Some(true),
            Choice::KeepAll => Some(false),
            _ => None,
        }
    }
}

impl Subject {
    pub fn question(self) -> &'static str {
        match self {
            Self::UnmanagedPath => "overwrite?",
            Self::Secret => "restore?",
            Self::Tool => "install?",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::UnmanagedPath => "UNMANAGED PATH",
            Self::Secret => "SECRET CHANGED",
            Self::Tool => "MISSING TOOL",
        }
    }
}
