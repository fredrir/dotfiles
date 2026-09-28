use std::path::PathBuf;

pub const SOCKET_NAME: &str = "op-bridge.sock";

pub fn broker() -> Result<PathBuf, String> {
    let base = match env_path("XDG_STATE_HOME") {
        Some(base) => base,
        None => env_path("HOME")
            .ok_or("HOME is not set")?
            .join(".local/state"),
    };
    Ok(base.join("op-bridge/broker.sock"))
}

pub fn client() -> PathBuf {
    if let Some(path) = env_path("OP_BRIDGE_SOCKET") {
        return path;
    }
    env_path("XDG_RUNTIME_DIR")
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", nix::unistd::getuid())))
        .join(SOCKET_NAME)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
