use serde::{Deserialize, Serialize};

use crate::decision::{Choice, Prompt};
use crate::event::Event;

pub const VERSION: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "kebab-case")]
pub enum Message {
    Hello {
        version: u32,
        host: String,
    },
    State {
        branch: String,
    },
    Change {
        value: String,
    },
    Ready,
    Phase {
        operation: String,
    },
    Log {
        operation: String,
        value: String,
    },
    Event {
        value: Event,
    },
    SyncReady {
        version: u32,
    },
    DecisionRequest {
        id: u64,
        prompt: Prompt,
    },
    DecisionResponse {
        id: u64,
        choice: Choice,
    },
    Completed,
    Error {
        operation: String,
        value: String,
        code: Option<i32>,
    },
    Continue,
    Discard,
    Cancel,
}

pub fn encode(message: &Message) -> Result<String, String> {
    serde_json::to_string(message).map_err(|error| format!("cannot encode push protocol: {error}"))
}

pub fn decode(line: &str) -> Result<Message, String> {
    serde_json::from_str(line).map_err(|error| format!("invalid push protocol frame: {error}"))
}
