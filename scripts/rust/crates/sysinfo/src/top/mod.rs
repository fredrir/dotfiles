//! Processes ranked by their share of the machine.

mod gpu;
pub mod group;
pub mod ps;
pub mod remote;
pub mod render;
mod sample;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

pub use sample::sample;

pub const SCHEMA: u32 = 1;
pub const WINDOW: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Each row's largest share of CPU, memory, or GPU.
    #[default]
    Total,
    Cpu,
    Memory,
    Gpu,
}

impl Sort {
    pub fn flag(self) -> Option<&'static str> {
        match self {
            Self::Total => None,
            Self::Cpu => Some("--cpu"),
            Self::Memory => Some("--memory"),
            Self::Gpu => Some("--gpu"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub sort: Sort,
    pub count: usize,
    pub split: bool,
}

/// One process as sampled across the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub parent: Option<u32>,
    pub uid: Option<u32>,
    pub name: String,
    pub exe: Option<String>,
    pub command: Vec<String>,
    pub kernel: bool,
    /// CPU milliseconds spent inside the window.
    pub cpu_ms: f64,
    pub memory: u64,
    /// Percent of all GPUs; `None` when the process holds no GPU work.
    pub gpu: Option<f64>,
    /// Seconds since start.
    pub age: u64,
}

/// What one sample saw of the machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub host: String,
    pub cores: usize,
    pub memory: u64,
    pub gpu: bool,
    pub window: Duration,
    pub processes: Vec<Process>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub host: String,
    pub cores: usize,
    pub memory: u64,
    pub gpu: bool,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub pid: u32,
    #[serde(skip)]
    pub uid: Option<u32>,
    pub user: String,
    /// Percent of all logical cores.
    pub cpu: f64,
    pub cores: f64,
    pub memory: u64,
    /// Percent of physical memory.
    pub memory_share: f64,
    pub gpu: Option<f64>,
    pub age: u64,
    pub command: String,
    pub count: usize,
}

impl Row {
    pub fn share(&self, sort: Sort) -> f64 {
        match sort {
            Sort::Total => self.cpu.max(self.memory_share).max(self.gpu.unwrap_or(0.0)),
            Sort::Cpu => self.cpu,
            Sort::Memory => self.memory_share,
            Sort::Gpu => self.gpu.unwrap_or(0.0),
        }
    }
}

pub fn report(sample: &Sample, options: Options) -> Report {
    let mut rows = group::rows(sample, options.split);
    group::rank(&mut rows, options.sort);
    rows.truncate(options.count);
    let mut users = HashMap::new();
    for row in &mut rows {
        row.user = user_name(row.uid, &mut users);
    }
    Report {
        schema: SCHEMA,
        host: sample.host.clone(),
        cores: sample.cores,
        memory: sample.memory,
        gpu: sample.gpu,
        rows,
    }
}

fn user_name(uid: Option<u32>, cache: &mut HashMap<u32, String>) -> String {
    let Some(uid) = uid else {
        return "?".into();
    };
    cache
        .entry(uid)
        .or_insert_with(|| {
            nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
                .ok()
                .flatten()
                .map_or_else(|| uid.to_string(), |user| user.name)
        })
        .clone()
}
