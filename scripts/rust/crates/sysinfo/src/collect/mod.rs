mod common;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", test))]
mod parse;

use crate::{inventory, model::Snapshot};
use hostkit::process::{CaptureLimits, output};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
static PROBES: AtomicUsize = AtomicUsize::new(0);
pub fn probe_count() -> usize {
    PROBES.load(Ordering::Relaxed)
}

pub fn executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let valid = |p: &Path| {
        p.is_file()
            && p.metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        let path = PathBuf::from(name);
        return valid(&path).then_some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(name))
        .find(|p| valid(p))
}
pub fn native_binary(name: &str) -> Result<PathBuf, String> {
    workstation::native::Resolver::discover(inventory::repo_root())
        .resolve(name)?
        .ok_or_else(|| format!("{name}: native binary not found; run ./setup.sh --commands-only"))
}
pub fn probe(command: &mut Command, timeout: Duration) -> Result<String, String> {
    PROBES.fetch_add(1, Ordering::Relaxed);
    let result = output(command, CaptureLimits::default(), timeout).map_err(|e| e.to_string())?;
    if !result.status.success() {
        let message = String::from_utf8_lossy(&result.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!(
                "{} exited {}",
                command.get_program().to_string_lossy(),
                result.status
            )
        } else {
            message
        });
    }
    if result.stdout_truncated {
        return Err("probe output exceeded its limit".into());
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().into())
}
pub fn index_modules(entries: Vec<Value>) -> Map<String, Value> {
    entries
        .into_iter()
        .filter_map(|value| {
            Some((
                value.get("type")?.as_str()?.to_string(),
                value.get("result")?.clone(),
            ))
        })
        .collect()
}
fn enrichment_modules() -> Vec<Value> {
    let mut modules = vec![
        json!("OS"),
        json!("Kernel"),
        json!("Shell"),
        json!({"type":"CPU","temp":true}),
        json!({"type":"GPU","temp":true,"driverSpecific":true}),
        json!("Memory"),
        json!("Swap"),
        json!("Disk"),
        json!("PhysicalMemory"),
        json!({"type":"PhysicalDisk","temp":true}),
        json!("DE"),
        json!("WM"),
        json!("Terminal"),
        json!("Board"),
        json!("Battery"),
        json!("PowerAdapter"),
    ];
    modules.extend(
        [
            "Host",
            "Uptime",
            "Packages",
            "CPUCache",
            "CPUUsage",
            "OpenCL",
            "Vulkan",
            "TerminalFont",
            "Theme",
            "WMTheme",
            "Display",
            "BIOS",
            "Bootmgr",
            "InitSystem",
        ]
        .map(|m| json!(m)),
    );
    modules
}
fn fastfetch(modules: Vec<Value>, trace: bool) -> Option<Vec<Value>> {
    if modules.is_empty() {
        return Some(Vec::new());
    }
    let executable = executable("fastfetch")?;
    use std::io::Write;
    let mut config = tempfile::Builder::new()
        .prefix("sysinfo-")
        .suffix(".jsonc")
        .tempfile()
        .ok()?;
    write!(config, "{}", json!({"modules":modules})).ok()?;
    let started = Instant::now();
    let result = probe(
        Command::new(executable)
            .arg("--config")
            .arg(config.path())
            .args(["--format", "json"]),
        Duration::from_secs(10),
    );
    if trace {
        eprintln!("fastfetch: {:?}", started.elapsed());
    }
    let text = result.ok()?;
    serde_json::from_str(&text).ok()
}
fn collect_modules(full: bool, trace: bool) -> Map<String, Value> {
    let started = Instant::now();
    let mut collected = Vec::new();
    common::collect(&mut collected);
    if trace {
        eprintln!("common: {:?}", started.elapsed());
    }
    #[cfg(target_os = "linux")]
    linux::collect(&mut collected);
    #[cfg(target_os = "macos")]
    macos::collect(&mut collected);
    if trace {
        eprintln!("platform: {:?}", started.elapsed());
    }
    let mut modules: Map<String, Value> = collected
        .into_iter()
        .map(|(kind, result)| (kind.into(), result))
        .collect();
    if full {
        let missing = enrichment_modules()
            .into_iter()
            .filter(|module| {
                let kind = module.as_str().or_else(|| module["type"].as_str());
                !kind.is_some_and(|kind| modules.contains_key(kind))
            })
            .collect();
        if let Some(extras) = fastfetch(missing, trace) {
            modules.extend(index_modules(extras));
        }
    }
    modules
}
pub fn shell_info() -> (String, String) {
    let path = std::env::var("SHELL").unwrap_or_default();
    let name = Path::new(&path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let expression = match name.as_str() {
        "zsh" => "printf %s \"$ZSH_VERSION\"",
        "bash" => "printf %s \"$BASH_VERSION\"",
        "fish" => "printf %s \"$version\"",
        _ => "",
    };
    let version = if expression.is_empty() {
        String::new()
    } else {
        probe(
            Command::new(path).args(["-c", expression]),
            Duration::from_secs(2),
        )
        .unwrap_or_default()
    };
    (name, version)
}
const TERMINALS: &[(&str, &str, &str)] = &[
    ("konsole", "Konsole", "konsole"),
    ("alacritty", "Alacritty", "alacritty"),
    ("wezterm", "WezTerm", "wezterm"),
    ("wezterm-gui", "WezTerm", "wezterm"),
    ("ghostty", "Ghostty", "ghostty"),
    ("foot", "foot", "foot"),
    ("footclient", "foot", "foot"),
    ("tilix", "Tilix", "tilix"),
    ("xfce4-terminal", "Xfce Terminal", "xfce4-terminal"),
];
pub fn terminal_info() -> (String, String) {
    let body = probe(
        Command::new("ps").args(["-ax", "-o", "pid=,ppid=,comm="]),
        Duration::from_secs(2),
    )
    .unwrap_or_default();
    let mut processes = HashMap::new();
    for line in body.lines() {
        let mut fields = line
            .trim()
            .splitn(3, char::is_whitespace)
            .filter(|v| !v.is_empty());
        // ps aligns numeric fields, so consume the two numbers before taking the command.
        let Some(pid_text) = fields.next() else {
            continue;
        };
        let rest = line
            .trim()
            .strip_prefix(pid_text)
            .unwrap_or("")
            .trim_start();
        let Some((parent, command)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        if let (Ok(pid), Ok(parent)) = (pid_text.parse::<u32>(), parent.parse::<u32>()) {
            processes.insert(pid, (parent, command.trim()));
        }
    }
    let mut ancestor = std::process::id();
    let mut visited = std::collections::HashSet::new();
    let mut terminal = None;
    while ancestor > 1 && visited.insert(ancestor) {
        let Some((parent, command)) = processes.get(&ancestor) else {
            break;
        };
        let process = Path::new(command)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        if process.starts_with("gnome-terminal") {
            terminal = Some(("GNOME Terminal", "gnome-terminal"));
            break;
        }
        if let Some((_, name, executable)) = TERMINALS.iter().find(|(comm, _, _)| *comm == process)
        {
            terminal = Some((*name, *executable));
            break;
        }
        ancestor = *parent;
    }
    let Some((name, executable)) = terminal else {
        return (String::new(), String::new());
    };
    let version = probe(
        Command::new(executable).arg("--version"),
        Duration::from_secs(2),
    )
    .unwrap_or_default()
    .lines()
    .next()
    .unwrap_or("")
    .split_whitespace()
    .nth(1)
    .unwrap_or("")
    .to_string();
    (name.into(), version)
}
pub fn recognized_terminal(value: &Value) -> bool {
    let identity = ["processName", "prettyName", "exeName", "exe"]
        .map(|k| value[k].as_str().unwrap_or(""))
        .join(" ")
        .to_lowercase();
    TERMINALS.iter().any(|(name, _, _)| identity.contains(name))
        || identity.contains("gnome-terminal")
        || identity.contains("gnome terminal")
}
fn versioned_name(name: String, version: String, fallback: &Value) -> String {
    let name = if name.is_empty() {
        fallback["prettyName"].as_str().unwrap_or("unknown").into()
    } else {
        name
    };
    let version = if version.is_empty() {
        fallback["version"].as_str().unwrap_or("").into()
    } else {
        version
    };
    if version.is_empty() {
        name
    } else {
        format!("{name} {version}")
    }
}
pub fn collect_nvidia() -> (Vec<Value>, String) {
    let Some(path) = executable("nvidia-smi") else {
        return (Vec::new(), String::new());
    };
    let fields = "index,name,memory.total,memory.used,utilization.gpu,temperature.gpu,power.draw,power.limit,clocks.current.graphics,driver_version";
    PROBES.fetch_add(1, Ordering::Relaxed);
    let result = match output(
        Command::new(path).args([
            &format!("--query-gpu={fields}"),
            "--format=csv,noheader,nounits",
        ]),
        CaptureLimits::default(),
        Duration::from_secs(3),
    ) {
        Ok(result) => result,
        Err(_) => return (Vec::new(), "NVIDIA live telemetry timed out".into()),
    };
    if !result.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
        .to_lowercase();
        return (
            Vec::new(),
            if text.contains("driver/library version mismatch") {
                "NVIDIA kernel driver does not match the installed userspace library"
            } else {
                "NVIDIA live telemetry is unavailable"
            }
            .into(),
        );
    }
    (parse_nvidia(&result.stdout), String::new())
}
pub fn parse_nvidia(bytes: &[u8]) -> Vec<Value> {
    // nvidia-smi separates fields with comma-space. CSV trimming happens after
    // quote recognition, so discard that initial space before parsing quoted names.
    let mut cleaned = Vec::with_capacity(bytes.len());
    let mut quoted = false;
    let mut field_start = true;
    for &byte in bytes {
        if field_start && byte == b' ' {
            continue;
        }
        cleaned.push(byte);
        if byte == b'"' {
            quoted = !quoted;
        }
        field_start = !quoted && matches!(byte, b',' | b'\n' | b'\r');
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(cleaned.as_slice());
    reader.records().filter_map(Result::ok).filter(|row|row.len()==10).map(|row| {
        let number=|index:usize|row[index].parse::<f64>().ok().filter(|v|v.is_finite());
        json!({"index":row[0].parse::<u32>().ok(),"name":row[1],"memory_total_mib":number(2),"memory_used_mib":number(3),"utilization":number(4),"temperature":number(5),"power_draw":number(6),"power_limit":number(7),"clock_mhz":number(8),"driver":row[9]})
    }).collect()
}
pub fn collect_snapshot(full: bool) -> Snapshot {
    collect_snapshot_with_timings(full, false)
}
pub fn collect_snapshot_with_timings(full: bool, timings: bool) -> Snapshot {
    collect_snapshot_in(full, timings, "", &inventory::InventoryContext::from_env())
}

pub fn collect_snapshot_for_host(
    full: bool,
    host: &str,
    context: &inventory::InventoryContext,
) -> Snapshot {
    collect_snapshot_in(full, false, host, context)
}

fn collect_snapshot_in(
    full: bool,
    timings: bool,
    host: &str,
    context: &inventory::InventoryContext,
) -> Snapshot {
    let started = Instant::now();
    let (modules, (shell_name, shell_version), (terminal_name, terminal_version)) =
        std::thread::scope(|scope| {
            let shell = scope.spawn(|| {
                let began = Instant::now();
                let result = shell_info();
                if timings {
                    eprintln!("shell: {:?}", began.elapsed());
                }
                result
            });
            let terminal = scope.spawn(|| {
                let began = Instant::now();
                let result = terminal_info();
                if timings {
                    eprintln!("terminal: {:?}", began.elapsed());
                }
                result
            });
            (
                collect_modules(full, timings),
                shell.join().unwrap_or_default(),
                terminal.join().unwrap_or_default(),
            )
        });
    let fallback = Value::Null;
    let module = |name: &str| modules.get(name).unwrap_or(&fallback);
    let terminal = if recognized_terminal(module("Terminal")) {
        module("Terminal")
    } else {
        &Value::Null
    };
    let de = module("DE");
    let wm = module("WM");
    let de_display = versioned_name(String::new(), String::new(), de);
    let mut wm_display = wm["prettyName"].as_str().unwrap_or("unknown").to_string();
    if let Some(protocol) = wm["protocolName"].as_str().filter(|p| !p.is_empty()) {
        wm_display.push_str(&format!(" ({protocol})"));
    }
    let has_nvidia = module("GPU").as_array().is_some_and(|gpus| {
        gpus.iter().any(|gpu| {
            let text = format!(
                "{} {}",
                gpu["vendor"].as_str().unwrap_or(""),
                gpu["name"].as_str().unwrap_or("")
            )
            .to_lowercase();
            text.contains("nvidia") || text.contains("geforce")
        })
    });
    let (nvidia, error) = if has_nvidia {
        collect_nvidia()
    } else {
        (Vec::new(), String::new())
    };
    let hardware = inventory::load_hosts_from(&context.hosts_file())
        .ok()
        .and_then(|hosts| {
            let mut name = inventory::resolve_with(context, &hosts, host, &[]);
            if name.is_empty() {
                name = inventory::match_hostname(&hosts, &inventory::local_hostnames());
            }
            hosts
                .into_iter()
                .find(|host| host.name == name)
                .map(|host| host.resolved_hardware())
        })
        .unwrap_or_else(inventory::default_hardware);
    if timings {
        eprintln!("collection: {:?}", started.elapsed());
    }
    Snapshot {
        hardware,
        shell_display: versioned_name(shell_name, shell_version, module("Shell")),
        terminal_display: versioned_name(terminal_name, terminal_version, terminal),
        de_display,
        wm_display,
        nvidia,
        probe_errors: if error.is_empty() {
            Vec::new()
        } else {
            vec![error]
        },
        modules,
    }
}
