use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache::{self, FirstUse, Source};
use crate::context::Context;
use crate::http::Request;
use crate::node::npmrc::DEFAULT_REGISTRY;
use crate::node::registry::{self, Hit};

// Broad searches whose most downloaded results make up the bare list.
const SEEDS: &[&str] = &[
    "keywords:cli",
    "cli",
    "command line",
    "typescript",
    "lint",
    "format",
    "test",
    "build",
    "bundler",
    "deploy",
    "ai",
    "agent",
    "package manager",
    "framework",
    "server",
    "monorepo",
];
// The registry refuses bursts of searches, so they go one at a time.
const SEARCH_GAP: Duration = Duration::from_secs(3);
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const CHECKED: usize = 400;
const CONCURRENCY: usize = 4;
const KEPT: usize = 150;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Popular {
    pub libraries: Vec<Hit>,
    // Packages whose latest version installs a command.
    pub tools: Vec<Hit>,
}

pub struct PopularSource;

impl Source for PopularSource {
    type Value = Popular;

    fn key(&self) -> String {
        "node-popular".into()
    }

    fn stamp(&self, _ctx: &Context) -> String {
        String::new()
    }

    fn ttl(&self) -> Option<Duration> {
        Some(Duration::from_secs(24 * 3600))
    }

    fn job(&self) -> Vec<String> {
        vec!["node-popular".into()]
    }

    fn first_use(&self) -> FirstUse {
        FirstUse::Background
    }

    fn build(&self, ctx: &Context) -> Option<Popular> {
        let mut pool: Vec<Hit> = Vec::new();
        for (index, seed) in SEEDS.iter().enumerate() {
            if index > 0 {
                thread::sleep(SEARCH_GAP);
            }
            match registry::query(ctx, DEFAULT_REGISTRY, seed, "250", SEARCH_TIMEOUT) {
                Some(hits) => pool.extend(hits),
                None => break,
            }
        }
        if pool.is_empty() {
            return None;
        }
        let ranked = ranked(pool);
        let tools = with_commands(ctx, &ranked[..ranked.len().min(CHECKED)]);
        Some(Popular {
            libraries: ranked.into_iter().take(KEPT).collect(),
            tools: tools.into_iter().take(KEPT).collect(),
        })
    }
}

pub fn load(ctx: &Context) -> Option<Popular> {
    if ctx.cached_packages {
        return cache::peek(ctx, "node-popular");
    }
    cache::load(ctx, &PopularSource)
}

pub fn ranked(pool: Vec<Hit>) -> Vec<Hit> {
    let mut unique: Vec<Hit> = Vec::new();
    for hit in pool {
        match unique.iter_mut().find(|known| known.name == hit.name) {
            Some(known) => known.downloads = known.downloads.max(hit.downloads),
            None => unique.push(hit),
        }
    }
    unique.sort_by_key(|hit| std::cmp::Reverse(hit.downloads));
    unique
}

fn with_commands(ctx: &Context, candidates: &[Hit]) -> Vec<Hit> {
    let mut tools = Vec::new();
    for chunk in candidates.chunks(CONCURRENCY) {
        let checked: Vec<bool> = thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|hit| scope.spawn(|| installs_command(ctx, &hit.name)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or(false))
                .collect()
        });
        tools.extend(
            chunk
                .iter()
                .zip(checked)
                .filter(|(_, command)| *command)
                .map(|(hit, _)| hit.clone()),
        );
    }
    tools
}

fn installs_command(ctx: &Context, name: &str) -> bool {
    let url = format!("{DEFAULT_REGISTRY}/{}/latest", name.replacen('/', "%2f", 1));
    Request::get(&url)
        .timeout(SEARCH_TIMEOUT)
        .json::<Value>(ctx)
        .is_some_and(|manifest| declares_command(&manifest))
}

pub fn declares_command(manifest: &Value) -> bool {
    match manifest.get("bin") {
        Some(Value::String(path)) => !path.is_empty(),
        Some(Value::Object(commands)) => !commands.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/node/popular_tests.rs"]
mod tests;
