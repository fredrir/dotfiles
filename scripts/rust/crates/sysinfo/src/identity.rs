use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

pub fn local_hostnames() -> Vec<String> {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(probe_hostnames).clone()
}
fn probe_hostnames() -> Vec<String> {
    crate::inventory::local_hostnames_with(sysinfo_backend::System::host_name().as_deref(), lookup)
}

/// Resolve a documented hostname probe in process where the platform exposes
/// the same value without a subprocess.
fn lookup(args: &[&str]) -> Option<String> {
    #[cfg(target_os = "macos")]
    if let ["scutil", "--get", key] = args {
        return crate::collect::macos::dynamic_store_name(key);
    }
    crate::collect::probe(
        Command::new(args[0]).args(&args[1..]),
        Duration::from_secs(2),
    )
    .ok()
}
pub fn display_hostname() -> String {
    std::env::var("SYSINFO_HOSTNAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            local_hostnames()
                .first()
                .map(|n| {
                    if cfg!(target_os = "macos") {
                        n.clone()
                    } else {
                        n.split('.').next().unwrap_or(n).into()
                    }
                })
                .unwrap_or_default()
        })
}
pub fn display_username() -> String {
    ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "unknown".into())
}
