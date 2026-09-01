use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Preflight,
    Tooling,
    Artifacts,
    Plan,
    Links,
    Secrets,
    Merge,
    Push,
    Remote,
    Integrations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Check,
    Create,
    Link,
    Prune,
    Merge,
    Secret,
    Generate,
    Push,
    Pull,
    Sync,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    Started {
        profile: String,
        dry_run: bool,
        peer: Option<String>,
    },
    PhaseStarted {
        phase: Phase,
        total: Option<usize>,
    },
    Progress {
        phase: Phase,
        completed: usize,
        total: Option<usize>,
        label: String,
    },
    Item {
        action: Action,
        path: PathBuf,
        detail: String,
        changed: bool,
    },
    Warning {
        message: String,
        hint: Option<String>,
    },
    Failed {
        phase: Phase,
        message: String,
        hint: Option<String>,
    },
    Finished(Summary),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub profile: String,
    #[serde(default)]
    pub peer: Option<String>,
    #[serde(default)]
    pub remote_changed: Option<usize>,
    pub checked: usize,
    pub changed: usize,
    pub links: usize,
    pub merges: usize,
    pub secrets: usize,
    pub generated: usize,
    pub dry_run: bool,
    pub elapsed: Duration,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

pub struct ChangeSetSink<'a> {
    inner: &'a dyn EventSink,
    paths: std::sync::Mutex<BTreeSet<PathBuf>>,
}

impl<'a> ChangeSetSink<'a> {
    pub fn new(inner: &'a dyn EventSink) -> Self {
        Self {
            inner,
            paths: std::sync::Mutex::new(BTreeSet::new()),
        }
    }

    pub fn changed(&self) -> usize {
        self.paths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

impl EventSink for ChangeSetSink<'_> {
    fn emit(&self, event: Event) {
        if let Event::Item {
            path,
            changed: true,
            ..
        } = &event
        {
            self.paths
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(path.clone());
        }
        self.inner.emit(event);
    }
}

impl EventSink for crossbeam_channel::Sender<Event> {
    fn emit(&self, event: Event) {
        let _ = self.send(event);
    }
}

#[derive(Default)]
pub struct VecSink(std::sync::Mutex<Vec<Event>>);

impl VecSink {
    pub fn events(&self) -> Vec<Event> {
        self.0
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl EventSink for VecSink {
    fn emit(&self, event: Event) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}
