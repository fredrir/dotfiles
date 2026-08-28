
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
