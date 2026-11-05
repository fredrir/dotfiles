//! Project order discovery through the project's own Tailwind installation.
//!
//! The bridge is intentionally optional: a missing Node, Tailwind package, or
//! project configuration is reported as a typed error so callers can fall back
//! to the Rust pattern sorter. Node is always a subprocess; project JavaScript
//! is never executed in this process.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::order::OrderTable;

const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

static TRUSTED: AtomicBool = AtomicBool::new(true);

/// Set by the editor host before project Tailwind JavaScript may be executed.
pub fn set_trusted(trusted: bool) {
    TRUSTED.store(trusted, Ordering::Relaxed);
}

/// Whether project-tailwind bridges are allowed in this process.
pub fn trusted() -> bool {
    TRUSTED.load(Ordering::Relaxed)
}
const RESULT_MARKER: &str = "__DOTFMT_TAILWIND_RESULT__";
const CACHE_VERSION: u32 = 1;
const MEMORY_CACHE_LIMIT: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("node is not available")]
    NodeMissing,
    #[error("tailwindcss is not installed in this project")]
    TailwindMissing,
    #[error("tailwind order bridge failed: {0}")]
    Failed(String),
}

/// Resolve the project's authoritative class order for the given classes.
///
/// Classes are cached per project, stylesheet/config mtimes, and Tailwind
/// version. Only classes absent from the cache are sent to Node; the resulting
/// table is merged back into memory and the XDG cache.
pub fn project_order(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    classes: &[String],
) -> Result<OrderTable, BridgeError> {
    project_order_with_cache(root, stylesheet, config, classes, &default_cache_root())
}

/// Merge conflicting utility classes through `tailwind-merge` when it is
/// installed in the project or a global Bun/npm location.
pub fn merge_class_list(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    classes: &str,
) -> Result<String, BridgeError> {
    let tokens = crate::sorter::split_classes(classes);
    if tokens.len() < 2 {
        return Ok(tokens.join(" "));
    }
    let project = Project::locate(root, stylesheet, config);
    if package_version("tailwind-merge", &project.root, &project.search_paths).is_none() {
        return Err(BridgeError::TailwindMissing);
    }
    let request = project.request("merge", tokens);
    let response = run_helper(&request, &node_binary(), BRIDGE_TIMEOUT)?;
    response
        .classes
        .ok_or_else(|| BridgeError::Failed("node bridge returned no merged classes".to_owned()))
}

fn project_order_with_cache(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    classes: &[String],
    cache_root: &Path,
) -> Result<OrderTable, BridgeError> {
    let project = Project::discover(root, stylesheet, config, cache_root);
    let mut table = project.load_table();
    if classes.is_empty() {
        return Ok(table);
    }

    let mut seen = HashSet::new();
    let unique: Vec<&str> = classes
        .iter()
        .map(String::as_str)
        .filter(|class| seen.insert(*class))
        .collect();
    let missing: Vec<&str> = unique
        .iter()
        .copied()
        .filter(|class| table.order(class).is_none())
        .collect();
    if missing.is_empty() {
        project.remember(&table);
        return Ok(table);
    }

    let request = project.request("order", missing.clone());
    let response = run_helper(&request, &node_binary(), BRIDGE_TIMEOUT)?;
    let mut returned: HashMap<String, Option<i64>> = response.orders.into_iter().collect();
    for class in &missing {
        let order = returned.remove(*class).ok_or_else(|| {
            BridgeError::Failed(format!("node bridge omitted the class {class:?}"))
        })?;
        table.insert(*class, order);
    }
    if table.source().is_empty() {
        table = table.with_source(project.source());
    }
    project.remember(&table);
    project.persist(&table);
    Ok(table)
}

fn default_cache_root() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("dotfmt").join("tailwind")
}

fn node_binary() -> OsString {
    std::env::var_os("DOTFMT_NODE").unwrap_or_else(|| OsString::from("node"))
}

struct Project {
    root: PathBuf,
    stylesheet: Option<PathBuf>,
    config: Option<PathBuf>,
    search_paths: Vec<PathBuf>,
    key: String,
    source: String,
    cache_file: PathBuf,
}

impl Project {
    fn locate(root: &Path, stylesheet: Option<&Path>, config: Option<&Path>) -> Self {
        let root = find_project_root(root);
        let stylesheet = stylesheet.map(|path| resolve_project_path(&root, path));
        let config = config.map(|path| resolve_project_path(&root, path));
        let search_paths = search_paths(&root);
        Self {
            root,
            stylesheet,
            config,
            search_paths,
            key: String::new(),
            source: String::new(),
            cache_file: PathBuf::new(),
        }
    }

    fn discover(
        root: &Path,
        stylesheet: Option<&Path>,
        config: Option<&Path>,
        cache_root: &Path,
    ) -> Self {
        let mut project = Self::locate(root, stylesheet, config);
        if project.stylesheet.is_none() {
            project.stylesheet = find_stylesheet(&project.root);
        }
        let config_for_key = project
            .config
            .clone()
            .or_else(|| find_config(&project.root));
        let tailwind_version = package_version("tailwindcss", &project.root, &project.search_paths);
        let plugin_version = package_version(
            "prettier-plugin-tailwindcss",
            &project.root,
            &project.search_paths,
        );
        project.key = cache_key(
            &project.root,
            project.stylesheet.as_deref(),
            config_for_key.as_deref(),
            &tailwind_version,
            &plugin_version,
        );
        project.source = format!(
            "project (tailwindcss {}, prettier-plugin-tailwindcss {})",
            tailwind_version.as_deref().unwrap_or("absent"),
            plugin_version.as_deref().unwrap_or("absent"),
        );
        project.cache_file = cache_root.join(format!("{}.json", hash_key(&project.key)));
        project
    }

    fn source(&self) -> String {
        self.source.clone()
    }

    fn request<'a>(&self, mode: &'static str, classes: Vec<&'a str>) -> HelperRequest<'a> {
        HelperRequest {
            mode,
            base: path_string(&self.root),
            resolve_paths: self
                .search_paths
                .iter()
                .map(|path| path_string(path))
                .collect(),
            stylesheet: self.stylesheet.as_deref().map(path_string),
            config: self.config.as_deref().map(path_string),
            classes,
        }
    }

    fn load_table(&self) -> OrderTable {
        if let Some(table) = memory_get(&self.key) {
            return table;
        }
        if let Some(cache) = read_cache(&self.cache_file, &self.key) {
            let table = OrderTable::from_pairs(cache.orders).with_source(cache.source);
            memory_put(&self.key, &table);
            return table;
        }
        OrderTable::empty().with_source(self.source())
    }

    fn remember(&self, table: &OrderTable) {
        memory_put(&self.key, table);
    }

    fn persist(&self, table: &OrderTable) {
        let additions = table
            .pairs()
            .map(|(class, order)| (class.to_owned(), order));
        update_cache(&self.cache_file, &self.key, &self.source(), additions);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HelperRequest<'a> {
    mode: &'static str,
    base: String,
    resolve_paths: Vec<String>,
    stylesheet: Option<String>,
    config: Option<String>,
    classes: Vec<&'a str>,
}

#[derive(Debug, Deserialize)]
struct HelperResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    orders: Vec<(String, Option<i64>)>,
    #[serde(default)]
    classes: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct DiskCache {
    version: u32,
    key: String,
    source: String,
    orders: BTreeMap<String, Option<i64>>,
}

fn find_project_root(start: &Path) -> PathBuf {
    let start = absolute(start);
    let mut current = if start.is_file() {
        start.parent().unwrap_or(&start).to_path_buf()
    } else {
        start.clone()
    };
    loop {
        if current.join("package.json").is_file() {
            return fs::canonicalize(&current).unwrap_or(current);
        }
        if !current.pop() {
            break;
        }
    }
    fs::canonicalize(&start).unwrap_or(start)
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn resolve_project_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// Find the shallowest CSS entry that declares a Tailwind v4 stylesheet.
///
/// Successful and failed discoveries are cached for the process lifetime; a
/// long-lived editor uses an explicit `tailwind_stylesheet` when it adds or
/// moves its entry point without restarting the server.
fn find_stylesheet(root: &Path) -> Option<PathBuf> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(found) = cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
    {
        return found.clone();
    }
    let found = find_stylesheet_uncached(root);
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(root.to_path_buf(), found.clone());
    found
}

fn find_stylesheet_uncached(root: &Path) -> Option<PathBuf> {
    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(6))
        .standard_filters(false)
        .filter_entry(|entry| {
            !entry
                .file_name()
                .to_str()
                .is_some_and(|name| matches!(name, "node_modules" | "dist" | "build" | ".git"))
        })
        .build();
    let mut found = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("css") {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if text.contains("@import \"tailwindcss\"")
            || text.contains("@tailwind")
            || text.contains("@theme")
            || text.contains("@utility")
        {
            found.push(path.to_path_buf());
        }
    }
    found.sort_by_key(|path| (path.components().count(), path.clone()));
    found.into_iter().next()
}

fn find_config(root: &Path) -> Option<PathBuf> {
    const NAMES: &[&str] = &[
        "tailwind.config.js",
        "tailwind.config.cjs",
        "tailwind.config.mjs",
        "tailwind.config.ts",
        "tailwind.config.cts",
        "tailwind.config.mts",
    ];
    let mut current = Some(root);
    while let Some(directory) = current {
        for name in NAMES {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        current = directory.parent();
    }
    None
}

fn search_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.to_path_buf()];
    for candidate in global_paths() {
        if candidate.is_dir() && !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    paths
}

fn global_paths() -> Vec<PathBuf> {
    let home = home_dir();
    let mut paths = Vec::new();
    if let Some(bun) = std::env::var_os("BUN_INSTALL").filter(|value| !value.is_empty()) {
        paths.push(PathBuf::from(bun).join("install/global/node_modules"));
    }
    if let Some(home) = &home {
        paths.push(home.join(".bun/install/global/node_modules"));
        paths.push(home.join(".npm-global/lib/node_modules"));
        paths.push(home.join(".local/lib/node_modules"));
    }
    for variable in ["NPM_CONFIG_PREFIX", "npm_config_prefix"] {
        if let Some(prefix) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            paths.push(PathBuf::from(prefix).join("lib/node_modules"));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if (directory.join("node").is_file() || directory.join("node.exe").is_file())
                && let Some(prefix) = directory.parent()
            {
                paths.push(prefix.join("lib/node_modules"));
            }
        }
    }
    if let Some(app_data) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        paths.push(PathBuf::from(app_data).join("npm/node_modules"));
    }
    paths.extend(
        [
            "/usr/local/lib/node_modules",
            "/usr/lib/node_modules",
            "/opt/homebrew/lib/node_modules",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    paths
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Package version from the project's own `node_modules` ancestry only.
fn local_package_version(name: &str, root: &Path) -> Option<String> {
    let mut current = Some(root);
    while let Some(directory) = current {
        let candidate = directory
            .join("node_modules")
            .join(name)
            .join("package.json");
        if let Some(version) = read_version(&candidate) {
            return Some(version);
        }
        current = directory.parent();
    }
    None
}

fn package_version(name: &str, root: &Path, search_paths: &[PathBuf]) -> Option<String> {
    let mut current = Some(root);
    while let Some(directory) = current {
        let candidate = directory
            .join("node_modules")
            .join(name)
            .join("package.json");
        if let Some(version) = read_version(&candidate) {
            return Some(version);
        }
        current = directory.parent();
    }
    for path in search_paths {
        let candidate = path.join(name).join("package.json");
        if let Some(version) = read_version(&candidate) {
            return Some(version);
        }
    }
    None
}

fn read_version(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let package: serde_json::Value = serde_json::from_str(&text).ok()?;
    package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn cache_key(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    tailwind_version: &Option<String>,
    plugin_version: &Option<String>,
) -> String {
    format!(
        "version={CACHE_VERSION}\nroot={}\nstylesheet={}:{}\nconfig={}:{}\ntailwind={}\nplugin={}",
        root.display(),
        stylesheet.map(path_string).unwrap_or_default(),
        file_stamp(stylesheet),
        config.map(path_string).unwrap_or_default(),
        file_stamp(config),
        tailwind_version.as_deref().unwrap_or("absent"),
        plugin_version.as_deref().unwrap_or("absent"),
    )
}

fn file_stamp(path: Option<&Path>) -> u128 {
    path.and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn hash_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hash, "{byte:02x}");
    }
    hash
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn memory_cache() -> &'static Mutex<HashMap<String, OrderTable>> {
    static CACHE: OnceLock<Mutex<HashMap<String, OrderTable>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn memory_get(key: &str) -> Option<OrderTable> {
    let cache = memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get(key).cloned()
}

fn memory_put(key: &str, table: &OrderTable) {
    let mut cache = memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !cache.contains_key(key) && cache.len() >= MEMORY_CACHE_LIMIT {
        cache.clear();
    }
    let entry = cache.entry(key.to_owned()).or_insert_with(|| table.clone());
    for (class, order) in table.pairs() {
        entry.insert(class, order);
    }
}

fn read_cache(path: &Path, key: &str) -> Option<DiskCache> {
    let text = fs::read_to_string(path).ok()?;
    let cache: DiskCache = serde_json::from_str(&text).ok()?;
    (cache.version == CACHE_VERSION && cache.key == key).then_some(cache)
}

fn write_cache(path: &Path, cache: &DiskCache) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(data) = serde_json::to_vec(cache) else {
        return;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("tailwind"),
        std::process::id(),
        nonce,
    ));
    if fs::write(&temporary, data).is_err() {
        return;
    }
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(path);
        let _ = fs::rename(&temporary, path);
    }
    let _ = fs::remove_file(&temporary);
}

struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    fn acquire(cache: &Path) -> Option<Self> {
        let path = cache.with_extension("lock");
        fs::create_dir_all(path.parent()?).ok()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Some(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(30));
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn update_cache(
    path: &Path,
    key: &str,
    source: &str,
    additions: impl IntoIterator<Item = (String, Option<i64>)>,
) {
    let _lock = CacheLock::acquire(path);
    let mut orders: BTreeMap<String, Option<i64>> = read_cache(path, key)
        .map(|cache| cache.orders)
        .unwrap_or_default();
    orders.extend(additions);
    let cache = DiskCache {
        version: CACHE_VERSION,
        key: key.to_owned(),
        source: source.to_owned(),
        orders,
    };
    write_cache(path, &cache);
}

fn run_helper(
    request: &HelperRequest<'_>,
    node: &OsStr,
    timeout: Duration,
) -> Result<HelperResponse, BridgeError> {
    let payload = serde_json::to_vec(request)
        .map_err(|error| BridgeError::Failed(format!("cannot encode bridge request: {error}")))?;
    let mut command = Command::new(node);
    command
        .arg("--input-type=module")
        .arg("-e")
        .arg(HELPER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.base.is_empty() {
        // Keep the process rooted in a valid directory when the project root is
        // unavailable; the helper still uses absolute paths from the request.
    } else if Path::new(&request.base).is_dir() {
        command.current_dir(&request.base);
    }
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => BridgeError::NodeMissing,
        _ => BridgeError::Failed(format!("cannot start node: {error}")),
    })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| BridgeError::Failed("node bridge has no stdin".to_owned()))?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BridgeError::Failed("node bridge has no stdout".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| BridgeError::Failed("node bridge has no stderr".to_owned()))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    let started = Instant::now();
    let mut timed_out = false;
    let mut wait_error = None;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                wait_error = Some(error);
                break;
            }
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = writer.join();
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if let Some(error) = wait_error {
        return Err(BridgeError::Failed(format!(
            "cannot wait for node bridge: {error}"
        )));
    }
    if timed_out {
        return Err(BridgeError::Failed(format!(
            "node bridge timed out after {}s",
            timeout.as_secs()
        )));
    }

    let response = parse_helper_response(&stdout, &stderr)?;
    if !response.ok {
        return Err(helper_failure(&response, &stderr));
    }
    Ok(response)
}

fn parse_helper_response(stdout: &[u8], stderr: &[u8]) -> Result<HelperResponse, BridgeError> {
    let stdout = String::from_utf8_lossy(stdout);
    let payload = stdout.lines().rev().find_map(|line| {
        let index = line.find(RESULT_MARKER)?;
        Some(&line[index + RESULT_MARKER.len()..])
    });
    let Some(payload) = payload else {
        return Err(BridgeError::Failed(format!(
            "node bridge produced no result{}",
            stderr_detail(stderr)
        )));
    };
    serde_json::from_str(payload).map_err(|error| {
        BridgeError::Failed(format!(
            "invalid node bridge response: {error}{}",
            stderr_detail(stderr)
        ))
    })
}

fn helper_failure(response: &HelperResponse, stderr: &[u8]) -> BridgeError {
    let kind = response.error.as_deref().unwrap_or("failed");
    let message = response.message.clone().unwrap_or_else(|| kind.to_owned());
    match kind {
        "tailwind-missing" => BridgeError::TailwindMissing,
        _ => BridgeError::Failed(format!("{message}{}", stderr_detail(stderr))),
    }
}

fn stderr_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let text: String = text.chars().take(2_000).collect();
    format!(": {text}")
}

const HELPER: &str = r#"
import { readFileSync } from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const MARKER = "__DOTFMT_TAILWIND_RESULT__";

function emit(value) {
  process.stdout.write(MARKER + JSON.stringify(value) + "\n");
}

function fail(kind, message) {
  emit({ ok: false, error: kind, message: String(message ?? kind) });
  process.exitCode = 1;
}

function normalizeOrders(pairs) {
  return pairs.map(([name, order]) => [
    name,
    typeof order === "bigint" ? Number(order) : (order ?? null),
  ]);
}

async function main() {
  let request;
  try {
    request = JSON.parse(readFileSync(0, "utf8"));
  } catch (error) {
    return fail("failed", `invalid bridge request: ${error}`);
  }

  const base = request.base;
  const bases = [base, ...(request.resolvePaths ?? [])].filter(
    (value) => typeof value === "string" && value.length > 0,
  );
  const resolvers = new Map();
  const resolverFor = (directory) => {
    let resolver = resolvers.get(directory);
    if (!resolver) {
      resolver = createRequire(pathToFileURL(path.join(directory, "package.json")));
      resolvers.set(directory, resolver);
    }
    return resolver;
  };
  const resolvePackage = (name) => {
    for (const directory of bases) {
      try {
        return resolverFor(directory).resolve(name);
      } catch {}
    }
    return null;
  };
  const importFile = (file) => import(pathToFileURL(file).href);
  const classes = Array.isArray(request.classes) ? request.classes : [];

  if (request.mode === "merge") {
    const mergePath = resolvePackage("tailwind-merge");
    if (!mergePath) return fail("tailwind-missing", "tailwind-merge is not installed");
    const mergeModule = await importFile(mergePath);
    const twMerge =
      mergeModule.twMerge ??
      mergeModule.default?.twMerge ??
      (typeof mergeModule.default === "function" ? mergeModule.default : null);
    if (typeof twMerge !== "function") {
      return fail("failed", "tailwind-merge does not export twMerge");
    }
    return emit({ ok: true, classes: twMerge(classes.join(" ")) });
  }

  const pluginPath =
    resolvePackage("prettier-plugin-tailwindcss/sorter") ??
    resolvePackage("prettier-plugin-tailwindcss");
  if (pluginPath) {
    const plugin = await importFile(pluginPath);
    const getTailwindConfig =
      plugin.getTailwindConfig ?? plugin.default?.getTailwindConfig;
    if (typeof getTailwindConfig === "function") {
      const api = await getTailwindConfig({
        base,
        configPath: request.config ?? undefined,
        stylesheetPath: request.stylesheet ?? undefined,
      });
      const orders = api.getClassOrder(classes);
      return emit({
        ok: true,
        api: "prettier-plugin-tailwindcss",
        orders: normalizeOrders(orders),
      });
    }
    // Older plugins expose createSorter but not getClassOrder. Try the
    // tailwindcss v4 API below instead of guessing an order from sorted output.
  }

  const tailwindPath = resolvePackage("tailwindcss");
  if (!tailwindPath) return fail("tailwind-missing", "tailwindcss is not installed");
  const tailwind = await importFile(tailwindPath);
  const tailwindModule = tailwind.default ?? tailwind;
  const loadDesignSystem =
    tailwind.__unstable__loadDesignSystem ??
    tailwindModule.__unstable__loadDesignSystem;
  if (typeof loadDesignSystem !== "function") {
    return fail(
      "unsupported",
      "tailwindcss v3 requires prettier-plugin-tailwindcss for class ordering",
    );
  }

  const resolveStyle = (id, from) => {
    if (id === "tailwindcss" || id.startsWith("tailwindcss/")) {
      let sub = id === "tailwindcss" ? "index.css" : id.slice("tailwindcss/".length);
      if (!sub.endsWith(".css")) sub += ".css";
      for (const directory of [from, ...bases]) {
        try {
          return resolverFor(directory).resolve("tailwindcss/" + sub);
        } catch {}
      }
    }
    if (id.startsWith(".") || path.isAbsolute(id)) return path.resolve(from, id);
    for (const directory of [from, ...bases]) {
      try {
        return resolverFor(directory).resolve(id);
      } catch {}
    }
    return path.resolve(from, id);
  };

  const css = request.stylesheet
    ? readFileSync(request.stylesheet, "utf8")
    : '@import "tailwindcss";';
  const importBase = request.stylesheet ? path.dirname(request.stylesheet) : base;
  const design = await loadDesignSystem(css, {
    base: importBase,
    loadStylesheet: async (id, from) => {
      const resolved = resolveStyle(id, from ?? importBase);
      return {
        base: path.dirname(resolved),
        content: readFileSync(resolved, "utf8"),
      };
    },
  });
  const orders = design.getClassOrder(classes);
  emit({ ok: true, api: "tailwindcss", orders: normalizeOrders(orders) });
}

main().catch((error) => fail("failed", error?.stack ?? error?.message ?? error));
"#;

/// Whether `root` belongs to a project that has Tailwind installed or configured.
///
/// This is a cheap filesystem check; it never starts Node.
pub fn is_tailwind_project(root: &Path) -> bool {
    let project = Project::locate(root, None, None);
    if find_config(&project.root).is_some() {
        return true;
    }
    if local_package_version("tailwindcss", &project.root).is_some()
        || local_package_version("prettier-plugin-tailwindcss", &project.root).is_some()
    {
        return true;
    }
    let mut current = Some(project.root.as_path());
    while let Some(directory) = current {
        let package = directory.join("package.json");
        if let Ok(text) = fs::read_to_string(package)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
        {
            for section in ["dependencies", "devDependencies", "peerDependencies"] {
                if value
                    .get(section)
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|dependencies| dependencies.contains_key("tailwindcss"))
                {
                    return true;
                }
            }
        }
        current = directory.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_available() -> bool {
        Command::new(node_binary())
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn empty_project() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp directory");
        fs::write(
            directory.path().join("package.json"),
            "{\"name\":\"fixture\"}\n",
        )
        .expect("package.json");
        directory
    }

    #[test]
    fn missing_node_maps_to_node_missing() {
        let project = empty_project();
        let request = HelperRequest {
            mode: "order",
            base: path_string(project.path()),
            resolve_paths: vec![path_string(project.path())],
            stylesheet: None,
            config: None,
            classes: vec!["p-4"],
        };
        let error = run_helper(
            &request,
            OsStr::new("/nonexistent/dotfmt-tailwind-node"),
            Duration::from_secs(1),
        )
        .expect_err("missing node");
        assert!(matches!(error, BridgeError::NodeMissing), "{error:?}");
    }

    #[test]
    fn missing_tailwind_maps_to_tailwind_missing() {
        if !node_available() {
            return;
        }
        let project = empty_project();
        let request = HelperRequest {
            mode: "order",
            base: path_string(project.path()),
            resolve_paths: vec![path_string(project.path())],
            stylesheet: None,
            config: None,
            classes: vec!["p-4"],
        };
        let error = run_helper(&request, &node_binary(), Duration::from_secs(20))
            .expect_err("missing tailwind");
        assert!(matches!(error, BridgeError::TailwindMissing), "{error:?}");
    }

    #[test]
    fn garbage_output_maps_to_failed() {
        let error = parse_helper_response(b"no result here\n", b"boom").expect_err("garbage");
        assert!(matches!(error, BridgeError::Failed(_)), "{error:?}");
    }

    #[test]
    fn direct_tailwind_fallback_uses_load_design_system() {
        if !node_available() {
            return;
        }
        let project = empty_project();
        let package = project.path().join("node_modules/tailwindcss");
        fs::create_dir_all(&package).expect("tailwind package");
        fs::write(
            package.join("package.json"),
            r#"{"name":"tailwindcss","version":"4.0.0","main":"index.js","exports":{".":"./index.js","./index.css":"./index.css"}}"#,
        )
        .expect("package.json");
        fs::write(
            package.join("index.js"),
            r#"exports.__unstable__loadDesignSystem = async (css, options) => {
  const sheet = await options.loadStylesheet("tailwindcss", options.base);
  if (!sheet.content.includes("--fake")) throw new Error("missing stylesheet");
  return { getClassOrder(classes) { return classes.map((name) => [name, name === "p-4" ? 3 : null]); } };
};"#,
        )
        .expect("index.js");
        fs::write(package.join("index.css"), "@theme { --fake: 1; }\n").expect("index.css");
        let request = HelperRequest {
            mode: "order",
            base: path_string(project.path()),
            resolve_paths: vec![path_string(project.path())],
            stylesheet: None,
            config: None,
            classes: vec!["p-4", "custom"],
        };
        let response = run_helper(&request, &node_binary(), Duration::from_secs(20))
            .expect("direct tailwind fallback");
        assert_eq!(
            response.orders,
            vec![("p-4".to_owned(), Some(3)), ("custom".to_owned(), None)]
        );
    }

    #[test]
    fn merge_helper_uses_tailwind_merge_when_installed() {
        if !node_available() {
            return;
        }
        let project = empty_project();
        let package = project.path().join("node_modules/tailwind-merge");
        fs::create_dir_all(&package).expect("tailwind-merge package");
        fs::write(
            package.join("package.json"),
            r#"{"name":"tailwind-merge","version":"3.0.0","main":"index.js"}"#,
        )
        .expect("package.json");
        fs::write(
            package.join("index.js"),
            r#"exports.twMerge = (classes) => classes.split(/\s+/).filter((name) => name !== "p-2").join(" ");"#,
        )
        .expect("index.js");
        let request = HelperRequest {
            mode: "merge",
            base: path_string(project.path()),
            resolve_paths: vec![path_string(project.path())],
            stylesheet: None,
            config: None,
            classes: vec!["p-2", "p-4"],
        };
        let response = run_helper(&request, &node_binary(), Duration::from_secs(20))
            .expect("tailwind-merge bridge");
        assert_eq!(response.classes.as_deref(), Some("p-4"));
    }

    #[test]
    fn v3_project_uses_plugin_when_available() {
        if !node_available() {
            return;
        }
        let project = empty_project();
        let config = project.path().join("tailwind.config.js");
        fs::write(&config, "module.exports = { content: [] };\n").expect("config");
        let package = project.path().join("node_modules/tailwindcss");
        fs::create_dir_all(&package).expect("tailwind package");
        fs::write(
            package.join("package.json"),
            r#"{"name":"tailwindcss","version":"3.4.0","main":"index.js"}"#,
        )
        .expect("package.json");
        fs::write(package.join("index.js"), "module.exports = {};\n").expect("index.js");
        let paths = search_paths(project.path());
        if package_version("prettier-plugin-tailwindcss", project.path(), &paths).is_none() {
            return;
        }
        let request = HelperRequest {
            mode: "order",
            base: path_string(project.path()),
            resolve_paths: paths.iter().map(|path| path_string(path)).collect(),
            stylesheet: None,
            config: Some(path_string(&config)),
            classes: vec!["p-4", "m-2", "custom"],
        };
        let response =
            run_helper(&request, &node_binary(), Duration::from_secs(20)).expect("v3 plugin order");
        assert_eq!(response.orders.len(), 3);
        assert!(
            response
                .orders
                .iter()
                .any(|(class, order)| class == "custom" && order.is_none())
        );
        assert!(
            response
                .orders
                .iter()
                .any(|(class, order)| class == "p-4" && order.is_some())
        );
    }

    #[test]
    fn cache_updates_merge_concurrently() {
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join("cache.json");
        let key = "concurrent-key";
        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    update_cache(
                        path,
                        key,
                        "test",
                        [(format!("class-{index}"), Some(index as i64))],
                    );
                });
            }
        });
        let cache = read_cache(&path, key).expect("cache");
        for index in 0..8 {
            assert_eq!(
                cache.orders.get(&format!("class-{index}")),
                Some(&Some(index as i64))
            );
        }
    }

    #[test]
    fn cache_key_tracks_stylesheet_mtime_and_versions() {
        let project = empty_project();
        let stylesheet = project.path().join("app.css");
        fs::write(&stylesheet, "a {}\n").expect("stylesheet");
        let root = project.path();
        let tailwind = Some("4.0.0".to_owned());
        let plugin = Some("0.8.0".to_owned());
        let first = cache_key(root, Some(&stylesheet), None, &tailwind, &plugin);
        std::thread::sleep(Duration::from_millis(5));
        fs::write(&stylesheet, "b {}\n").expect("stylesheet");
        let second = cache_key(root, Some(&stylesheet), None, &tailwind, &plugin);
        assert_ne!(first, second);
        let other_version = Some("4.1.0".to_owned());
        assert_ne!(
            second,
            cache_key(root, Some(&stylesheet), None, &other_version, &plugin)
        );
    }

    #[test]
    fn project_order_fixture_when_node_and_tailwind_are_available() {
        if !node_available() {
            return;
        }
        let project = empty_project();
        let stylesheet = project.path().join("app.css");
        fs::write(&stylesheet, "@import \"tailwindcss\";\n").expect("stylesheet");
        let cache = tempfile::tempdir().expect("cache directory");
        let classes = vec![
            "p-4".to_owned(),
            "m-2".to_owned(),
            "flex".to_owned(),
            "hover:bg-red-500".to_owned(),
            "custom".to_owned(),
            "...".to_owned(),
        ];
        let table = match project_order_with_cache(
            project.path(),
            Some(Path::new("app.css")),
            None,
            &classes,
            cache.path(),
        ) {
            Ok(table) => table,
            Err(BridgeError::NodeMissing | BridgeError::TailwindMissing) => return,
            Err(error) => panic!("unexpected bridge error: {error:?}"),
        };
        let order = |class: &str| table.order(class).flatten();
        let m2 = order("m-2").expect("m-2 order");
        let flex = order("flex").expect("flex order");
        let p4 = order("p-4").expect("p-4 order");
        let hover = order("hover:bg-red-500").expect("variant order");
        assert!(m2 < flex && flex < p4 && p4 < hover, "{table:?}");
        assert_eq!(table.order("custom"), Some(None));
        assert_eq!(table.order("..."), Some(None));
        assert_eq!(
            crate::sorter::sort_class_list("p-4 custom m-2 flex ...", &table, true),
            "custom m-2 flex p-4 ..."
        );
        memory_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        let cached = project_order_with_cache(
            project.path(),
            Some(Path::new("app.css")),
            None,
            &["gap-2".to_owned(), "custom".to_owned()],
            cache.path(),
        )
        .expect("cached order");
        assert_eq!(cached.order("m-2"), Some(Some(m2)));
        assert_eq!(cached.order("custom"), Some(None));
        assert!(cached.order("gap-2").flatten().is_some());
        let cached_file = fs::read_dir(cache.path())
            .expect("cache directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .expect("cache file");
        let disk: DiskCache =
            serde_json::from_str(&fs::read_to_string(cached_file).expect("cache contents"))
                .expect("cache JSON");
        assert!(disk.orders.contains_key("gap-2"));
        assert!(disk.orders.contains_key("m-2"));
    }
}
