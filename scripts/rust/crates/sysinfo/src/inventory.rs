use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use workstation::blocks::{self, Comments};

pub const HARDWARE_KEYS: &[(&str, &str)] = &[
    ("GPU", "gpu"),
    ("CPU", "cpu"),
    ("CPU_COOLER", "cpu_cooler"),
    ("MOTHERBOARD", "motherboard"),
    ("MEMORY", "memory"),
    ("STORAGE", "storage"),
    ("CASE", "case"),
    ("POWER_SUPPLY", "power_supply"),
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub name: String,
    pub hostnames: Vec<String>,
    pub role: String,
    pub hardware: BTreeMap<String, String>,
}
impl Host {
    pub fn resolved_hardware(&self) -> BTreeMap<String, String> {
        let mut values = default_hardware();
        values.extend(self.hardware.clone());
        values
    }
}
pub fn default_hardware() -> BTreeMap<String, String> {
    ["cpu_cooler", "case", "power_supply"]
        .into_iter()
        .map(|key| (key.into(), "not set".into()))
        .collect()
}

#[derive(Clone, Debug)]
pub struct InventoryContext {
    pub root: PathBuf,
    pub host: Option<String>,
    pub config: Option<PathBuf>,
    pub state_file: PathBuf,
}
impl InventoryContext {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Self {
            root: repo_root(),
            state_file: config_home.join("dotfile/host"),
            host: std::env::var("SYSINFO_HOST").ok().filter(|v| !v.is_empty()),
            config: std::env::var_os("SYSINFO_CONFIG")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }
    pub fn hosts_file(&self) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(|| self.root.join("config/hosts.dotfile"))
    }
}

pub fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("DOTFILE_ROOT").filter(|v| !v.is_empty()) {
        return PathBuf::from(root);
    }
    let executable = std::env::current_exe().ok();
    let compiled = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for start in executable.iter().chain(std::iter::once(&compiled)) {
        for path in start.ancestors() {
            if path.join("config/targets.dotfile").is_file() || path.join(".git").exists() {
                return path.to_path_buf();
            }
        }
    }
    compiled
        .join("../../../..")
        .canonicalize()
        .unwrap_or(compiled)
}

pub fn parse_hosts(text: &str) -> Result<Vec<Host>, String> {
    let mut hosts: Vec<Host> = Vec::new();
    for entry in blocks::parse_with_comments(text, Comments::Lines)? {
        if !valid_name(&entry.block) {
            return Err(format!(
                "line {}: invalid host name: {}",
                entry.number, entry.block
            ));
        }
        let index = match hosts.iter().position(|host| host.name == entry.block) {
            Some(index) => index,
            None => {
                hosts.push(Host {
                    name: entry.block.clone(),
                    ..Host::default()
                });
                hosts.len() - 1
            }
        };
        if entry.opens {
            continue;
        }
        if !entry.text.contains('=') {
            return Err(format!("line {}: host field requires '='", entry.number));
        }
        let (key, value) = entry.split();
        let key = key.split_whitespace().collect::<String>().to_uppercase();
        match key.as_str() {
            "HOSTNAMES" => {
                hosts[index].hostnames = value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            "ROLE" => hosts[index].role = value.into(),
            _ => {
                if let Some((_, field)) = HARDWARE_KEYS.iter().find(|(name, _)| *name == key) {
                    hosts[index].hardware.insert((*field).into(), value.into());
                }
            }
        }
    }
    Ok(hosts)
}
pub fn load_hosts_from(path: &Path) -> Result<Vec<Host>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    parse_hosts(&text).map_err(|error| format!("{}: {error}", path.display()))
}
pub fn load_hosts() -> Result<Vec<Host>, String> {
    load_hosts_from(&InventoryContext::from_env().hosts_file())
}
pub fn match_hostname(hosts: &[Host], names: &[String]) -> String {
    hosts
        .iter()
        .find(|host| {
            std::iter::once(&host.name)
                .chain(host.hostnames.iter())
                .any(|alias| names.iter().any(|name| name.eq_ignore_ascii_case(alias)))
        })
        .map(|host| host.name.clone())
        .unwrap_or_default()
}
pub fn resolve_with(
    context: &InventoryContext,
    hosts: &[Host],
    explicit: &str,
    local_names: &[String],
) -> String {
    if !explicit.is_empty() {
        return explicit.into();
    }
    if let Some(host) = context.host.as_ref().filter(|s| !s.is_empty()) {
        return host.clone();
    }
    if let Ok(pin) = fs::read_to_string(&context.state_file)
        && !pin.trim().is_empty()
    {
        return pin.trim().into();
    }
    match_hostname(hosts, local_names)
}
pub fn resolve(explicit: &str) -> Result<String, String> {
    let context = InventoryContext::from_env();
    // Explicit or pinned identities do not require parsing an unrelated inventory.
    let resolved = resolve_with(&context, &[], explicit, &[]);
    if !resolved.is_empty() {
        return Ok(resolved);
    }
    Ok(match_hostname(
        &load_hosts_from(&context.hosts_file())?,
        &local_hostnames(),
    ))
}
pub fn local_hostnames() -> Vec<String> {
    crate::identity::local_hostnames()
}

pub fn local_hostnames_with(
    hostname: Option<&str>,
    mut probe: impl FnMut(&[&str]) -> Option<String>,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |name: &str| {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
        }
    };
    if cfg!(target_os = "macos") {
        for key in ["LocalHostName", "ComputerName"] {
            if let Some(name) = probe(&["scutil", "--get", key]) {
                push(&name);
            }
        }
    }
    let queried = if hostname.is_none() {
        probe(&["hostname"])
    } else {
        None
    };
    if let Some(raw) = hostname.or(queried.as_deref()) {
        push(raw);
        push(raw.trim().split('.').next().unwrap_or(""));
    }
    names
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
pub fn save_host(name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err("host must be a valid single path component".into());
    }
    let path = InventoryContext::from_env().state_file;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, format!("{name}\n")).map_err(|e| e.to_string())
}
pub fn render_host(host: &Host) -> String {
    let mut out = format!("{} {{\n", host.name);
    if !host.hostnames.is_empty() {
        out.push_str(&format!("  hostnames = {}\n", host.hostnames.join(", ")));
    }
    if !host.role.is_empty() {
        out.push_str(&format!("  role = {}\n", host.role));
    }
    if host.hardware.values().any(|v| !v.is_empty()) {
        out.push('\n');
    }
    for (label, key) in HARDWARE_KEYS {
        if let Some(value) = host.hardware.get(*key).filter(|v| !v.is_empty()) {
            out.push_str(&format!("  {label} = {value}\n"));
        }
    }
    out.push_str("}\n");
    out
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
    let path = InventoryContext::from_env().hosts_file();
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
