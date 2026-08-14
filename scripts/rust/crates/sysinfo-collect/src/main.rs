//! Native system probing that emits fastfetch-shaped module JSON.
//!
//! The Python sysinfo tool historically shelled out to fastfetch and consumed
//! its `[{type, result}, ...]` JSON. This binary produces the same shape for
//! the modules that decide machine identity and benchmark gating, so those
//! paths work with no external tool. Purely cosmetic modules (Host, Packages,
//! Theme, Display, ...) are left to fastfetch, which the Python side merges
//! in when it is installed.
//!
//! Field values follow what fastfetch reports on the same hardware, because
//! the benchmark epoch (machine identity) is derived from them: a renamed CPU
//! or disk would orphan every pinned baseline.

mod common;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod parse;

use serde_json::Value;

pub type Module = (&'static str, Value);

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("sysinfo-collect {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let trace = std::env::var("SYSINFO_COLLECT_TRACE").is_ok();
    let started = std::time::Instant::now();
    let mut modules: Vec<Module> = Vec::new();
    common::collect(&mut modules);
    if trace {
        eprintln!("common: {:?}", started.elapsed());
    }
    #[cfg(target_os = "linux")]
    linux::collect(&mut modules);
    #[cfg(target_os = "macos")]
    macos::collect(&mut modules);
    if trace {
        eprintln!("platform: {:?}", started.elapsed());
    }
    let listed: Vec<Value> = modules
        .into_iter()
        .map(|(kind, result)| serde_json::json!({"type": kind, "result": result}))
        .collect();
    println!("{}", Value::Array(listed));
}
