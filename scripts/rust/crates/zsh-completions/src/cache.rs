use std::fs::{self, OpenOptions};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::context::{Context, secs};

const LOCK_EXPIRY: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FirstUse {
    Build,
    Background,
}

pub trait Source {
    type Value: Serialize + DeserializeOwned;

    fn key(&self) -> String;
    fn stamp(&self, ctx: &Context) -> String;
    fn ttl(&self) -> Option<Duration>;
    // Arguments to `zcomp refresh` that rebuild this source.
    fn job(&self) -> Vec<String>;
    fn build(&self, ctx: &Context) -> Option<Self::Value>;

    fn first_use(&self) -> FirstUse {
        FirstUse::Build
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    stamp: String,
    at: u64,
    value: T,
}

pub fn load<S: Source>(ctx: &Context, source: &S) -> Option<S::Value> {
    let key = source.key();
    let stamp = source.stamp(ctx);
    let stored = read::<S::Value>(ctx, &key);
    if is_fresh(ctx, source, stored.as_ref()) {
        return stored.map(|stored| stored.value);
    }
    let build_now = ctx.foreground || (stored.is_none() && source.first_use() == FirstUse::Build);
    if build_now {
        return match source.build(ctx) {
            Some(value) => {
                write(ctx, &key, &stamp, &value);
                Some(value)
            }
            None => stored.map(|stored| stored.value),
        };
    }
    spawn_refresh(ctx, &key, &source.job());
    stored.map(|stored| stored.value)
}

// Starts a background rebuild when the stored value is missing or stale.
pub fn warm<S: Source>(ctx: &Context, source: &S) {
    let stored = read::<S::Value>(ctx, &source.key());
    if !is_fresh(ctx, source, stored.as_ref()) {
        spawn_refresh(ctx, &source.key(), &source.job());
    }
}

fn is_fresh<S: Source>(ctx: &Context, source: &S, stored: Option<&Envelope<S::Value>>) -> bool {
    stored.is_some_and(|stored| {
        stored.stamp == source.stamp(ctx)
            && source
                .ttl()
                .is_none_or(|ttl| ctx.now_secs().saturating_sub(stored.at) < ttl.as_secs())
    })
}

pub fn refresh<S: Source>(ctx: &Context, source: &S) -> bool {
    let key = source.key();
    let built = source.build(ctx);
    if let Some(value) = &built {
        write(ctx, &key, &source.stamp(ctx), value);
    }
    let _ = fs::remove_file(lock_path(ctx, &key));
    built.is_some()
}

pub fn peek<T: DeserializeOwned>(ctx: &Context, key: &str) -> Option<T> {
    read(ctx, key).map(|stored: Envelope<T>| stored.value)
}

// The stored value with its age in seconds.
pub fn peek_aged<T: DeserializeOwned>(ctx: &Context, key: &str) -> Option<(T, u64)> {
    read(ctx, key)
        .map(|stored: Envelope<T>| (stored.value, ctx.now_secs().saturating_sub(stored.at)))
}

pub fn store<T: Serialize>(ctx: &Context, key: &str, value: &T) {
    write(ctx, key, "", value);
}

fn read<T: DeserializeOwned>(ctx: &Context, key: &str) -> Option<Envelope<T>> {
    let bytes = fs::read(entry_path(ctx, key)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write<T: Serialize>(ctx: &Context, key: &str, stamp: &str, value: &T) {
    let path = entry_path(ctx, key);
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let envelope = Envelope {
        stamp: stamp.to_string(),
        at: secs(ctx.now),
        value,
    };
    let Ok(bytes) = serde_json::to_vec(&envelope) else {
        return;
    };
    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
    if fs::write(&staging, bytes).is_ok() && fs::rename(&staging, &path).is_err() {
        let _ = fs::remove_file(&staging);
    }
}

fn spawn_refresh(ctx: &Context, key: &str, job: &[String]) {
    if job.is_empty() || !take_lock(ctx, key) {
        return;
    }
    let Ok(program) = std::env::current_exe() else {
        let _ = fs::remove_file(lock_path(ctx, key));
        return;
    };
    let spawned = Command::new(program)
        .arg("refresh")
        .args(job)
        .current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    if spawned.is_err() {
        let _ = fs::remove_file(lock_path(ctx, key));
    }
}

fn take_lock(ctx: &Context, key: &str) -> bool {
    let path = lock_path(ctx, key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let expired = fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| ctx.now.duration_since(modified).ok())
        .is_some_and(|held| held > LOCK_EXPIRY);
    if expired {
        let _ = fs::remove_file(&path);
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .is_ok()
}

fn entry_path(ctx: &Context, key: &str) -> PathBuf {
    ctx.cache_dir().join(format!("{}.json", file_name(key)))
}

fn lock_path(ctx: &Context, key: &str) -> PathBuf {
    ctx.cache_dir().join(format!("{}.lock", file_name(key)))
}

// Keys stay readable on disk and distinct keys never share a file.
fn file_name(key: &str) -> String {
    let mut name = String::with_capacity(key.len());
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.') {
            name.push(byte as char);
        } else {
            name.push_str(&format!("_{byte:02x}"));
        }
    }
    name
}

#[cfg(test)]
#[path = "../tests/unit/cache_tests.rs"]
mod tests;
