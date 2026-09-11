use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use sysinfo::inventory::{Host, InventoryContext, render_host, valid_name};

pub fn save_host(name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err("host must be a valid single path component".into());
    }
    let path = inventory_context().state_file;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, format!("{name}\n")).map_err(|e| e.to_string())
}
pub fn append_host(host: &Host) -> Result<PathBuf, String> {
    if !valid_name(&host.name) {
        return Err("host must be a valid single path component".into());
    }
    if host
        .hostnames
        .iter()
        .chain(std::iter::once(&host.role))
        .chain(host.hardware.values())
        .any(|value| value.contains(['\n', '\r', '{', '}']))
    {
        return Err("host fields must be single lines without braces".into());
    }
    let path = inventory_context().hosts_file();
    let previous = fs::read_to_string(&path).unwrap_or_default();
    let prefix = if previous.is_empty() || previous.ends_with("\n\n") {
        ""
    } else if previous.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(format!("{prefix}{}", render_host(host)).as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn inventory_context() -> InventoryContext {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    InventoryContext {
        root: sysinfo::inventory::repo_root(),
        host: None,
        config: None,
        state_file: config_home.join("dotfile/host"),
    }
}

pub fn load_hosts() -> Result<Vec<Host>, String> {
    sysinfo::inventory::load_hosts_from(&inventory_context().hosts_file())
}
