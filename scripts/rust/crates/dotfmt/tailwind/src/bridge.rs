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
const CACHE_VERSION: u32 = 2;
const MEMORY_CACHE_LIMIT: usize = 64;
const DISCOVERY_TTL: Duration = Duration::from_millis(750);
const DEPENDENCY_FILE_LIMIT: usize = 512;
const DEPENDENCY_BYTE_LIMIT: u64 = 8 * 1024 * 1024;
const DEPENDENCY_ENTRY_LIMIT: usize = 50_000;
const DEPENDENCY_LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
];
const TAILWIND_CONFIG_NAMES: &[&str] = &[
    "tailwind.config.js",
    "tailwind.config.cjs",
    "tailwind.config.mjs",
    "tailwind.config.ts",
    "tailwind.config.cts",
    "tailwind.config.mts",
];
const CONVENTIONAL_STYLESHEETS: &[&str] = &[
    "src/index.css",
    "src/app.css",
    "app/globals.css",
    "src/styles/globals.css",
];

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("node is not available")]
    NodeMissing,
    #[error("tailwindcss is not installed in this project")]
    TailwindMissing,
    #[error("tailwind order bridge failed: {0}")]
    Failed(String),
    #[error("ambiguous Tailwind stylesheet; candidates: {0:?}")]
    AmbiguousStylesheet(Vec<PathBuf>),
    #[error("configured Tailwind stylesheet does not exist: {}", .0.display())]
    MissingStylesheet(PathBuf),
    #[error("configured Tailwind config does not exist: {}", .0.display())]
    MissingConfig(PathBuf),
}

/// The deterministic Tailwind project selected for one source file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    pub package_root: PathBuf,
    pub workspace_root: PathBuf,
    pub stylesheet: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub tailwind_version: Option<String>,
    pub plugin_version: Option<String>,
    /// Non-fatal configuration choices (for example both path settings set).
    pub warnings: Vec<String>,
    /// CSS entries imported by the selected stylesheet or the source file.
    pub dependencies: Vec<PathBuf>,
}

/// A resolved project order together with the discovery it came from.
#[derive(Clone, Debug)]
pub struct ProjectOrder {
    pub table: OrderTable,
    pub discovery: Discovery,
}

/// Resolve the project's authoritative class order for the given classes.
///
/// Results are cached per theme (discovery result, versions, mtimes, and a
/// bounded dependency digest) and per exact candidate set. A cache miss sends
/// the file's complete unique class list to Node; subset ranks are never merged
/// into an existing table.
pub fn project_order(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    classes: &[String],
) -> Result<OrderTable, BridgeError> {
    project_order_with_cache(root, stylesheet, config, classes, &default_cache_root())
}

/// Resolve the order for `source`, with explicit paths resolved against the
/// configuration layer that declared them (`None` means the workspace root).
pub fn project_order_for(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
    classes: &[String],
) -> Result<ProjectOrder, BridgeError> {
    project_order_with_cache_for(
        root,
        source,
        stylesheet,
        stylesheet_base,
        config,
        config_base,
        classes,
        &default_cache_root(),
    )
}

/// Discover the selected project without starting Node.
pub fn discover_project(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
) -> Result<Discovery, BridgeError> {
    discover_project_cached(
        root,
        source,
        stylesheet,
        stylesheet_base,
        config,
        config_base,
    )
}

/// Clear process-local discovery and order caches after watched files change.
pub fn invalidate_caches() {
    discovery_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
    memory_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
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
    project_order_with_cache_using(
        root,
        stylesheet,
        config,
        classes,
        cache_root,
        |project, classes| {
            let request = project.request("order", classes);
            run_helper(&request, &node_binary(), BRIDGE_TIMEOUT)
        },
    )
    .map(|order| order.table)
}

#[allow(clippy::too_many_arguments)]
fn project_order_with_cache_for(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
    classes: &[String],
    cache_root: &Path,
) -> Result<ProjectOrder, BridgeError> {
    project_order_with_cache_for_using(
        root,
        source,
        stylesheet,
        stylesheet_base,
        config,
        config_base,
        classes,
        cache_root,
        |project, classes| {
            let request = project.request("order", classes);
            run_helper(&request, &node_binary(), BRIDGE_TIMEOUT)
        },
    )
}

fn project_order_with_cache_using<F>(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    classes: &[String],
    cache_root: &Path,
    query: F,
) -> Result<ProjectOrder, BridgeError>
where
    F: Fn(&Project, Vec<&str>) -> Result<HelperResponse, BridgeError>,
{
    project_order_with_cache_for_using(
        root, None, stylesheet, None, config, None, classes, cache_root, query,
    )
}

#[allow(clippy::too_many_arguments)]
fn project_order_with_cache_for_using<F>(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
    classes: &[String],
    cache_root: &Path,
    query: F,
) -> Result<ProjectOrder, BridgeError>
where
    F: Fn(&Project, Vec<&str>) -> Result<HelperResponse, BridgeError>,
{
    let project = Project::discover(
        root,
        source,
        stylesheet,
        stylesheet_base,
        config,
        config_base,
        cache_root,
    )?;
    let discovery = project.discovery();
    let table = project_order_with_project(project, classes, query)?;
    Ok(ProjectOrder { table, discovery })
}

fn project_order_with_project<F>(
    project: Project,
    classes: &[String],
    query: F,
) -> Result<OrderTable, BridgeError>
where
    F: Fn(&Project, Vec<&str>) -> Result<HelperResponse, BridgeError>,
{
    let mut seen = HashSet::new();
    let unique: Vec<&str> = classes
        .iter()
        .map(String::as_str)
        .filter(|class| seen.insert(*class))
        .collect();
    if unique.is_empty() {
        return Ok(OrderTable::empty().with_source(project.source()));
    }

    let candidate_set = candidate_set_hash(&unique);
    if let Some(table) = project.load_candidate(&candidate_set) {
        return Ok(table);
    }

    // getClassOrder ranks are relative to the queried list. Always ask for the
    // file's complete candidate set and store only that complete result.
    let response = query(&project, unique.clone())?;
    let mut returned: HashMap<String, Option<i64>> = response.orders.into_iter().collect();
    let mut table = OrderTable::empty().with_source(project.source());
    for class in &unique {
        let order = returned.remove(*class).ok_or_else(|| {
            BridgeError::Failed(format!("node bridge omitted the class {class:?}"))
        })?;
        table.insert(*class, order);
    }
    project.remember_candidate(&candidate_set, &table);
    project.persist_candidate(&candidate_set, &table);
    Ok(table)
}

fn candidate_set_hash(classes: &[&str]) -> String {
    let mut sorted = classes.to_vec();
    sorted.sort_unstable();
    let mut hasher = Sha256::new();
    for class in sorted {
        hasher.update((class.len() as u64).to_le_bytes());
        hasher.update(class.as_bytes());
    }
    hex_digest(hasher.finalize())
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
    workspace_root: PathBuf,
    stylesheet: Option<PathBuf>,
    config: Option<PathBuf>,
    search_paths: Vec<PathBuf>,
    key: String,
    source: String,
    cache_root: PathBuf,
    persistent: bool,
    tailwind_version: Option<String>,
    plugin_version: Option<String>,
    warnings: Vec<String>,
    dependencies: Vec<PathBuf>,
}

impl Project {
    fn locate(root: &Path, stylesheet: Option<&Path>, config: Option<&Path>) -> Self {
        let root = find_project_root(root);
        let workspace_root = find_workspace_root(&root);
        let stylesheet = stylesheet.map(|path| resolve_project_path(&root, path));
        let config = config.map(|path| resolve_project_path(&root, path));
        let search_paths = search_paths(&root);
        Self {
            root,
            workspace_root,
            stylesheet,
            config,
            search_paths,
            key: String::new(),
            source: String::new(),
            cache_root: PathBuf::new(),
            persistent: false,
            tailwind_version: None,
            plugin_version: None,
            warnings: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn discover(
        root: &Path,
        source: Option<&Path>,
        stylesheet: Option<&Path>,
        stylesheet_base: Option<&Path>,
        config: Option<&Path>,
        config_base: Option<&Path>,
        cache_root: &Path,
    ) -> Result<Self, BridgeError> {
        Self::discover_with_limits(
            root,
            source,
            stylesheet,
            stylesheet_base,
            config,
            config_base,
            cache_root,
            DEPENDENCY_FILE_LIMIT,
            DEPENDENCY_BYTE_LIMIT,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn discover_with_limits(
        root: &Path,
        source: Option<&Path>,
        stylesheet: Option<&Path>,
        stylesheet_base: Option<&Path>,
        config: Option<&Path>,
        config_base: Option<&Path>,
        cache_root: &Path,
        dependency_file_limit: usize,
        dependency_byte_limit: u64,
    ) -> Result<Self, BridgeError> {
        let discovery = discover_project_cached(
            root,
            source,
            stylesheet,
            stylesheet_base,
            config,
            config_base,
        )?;
        let search_paths = search_paths(&discovery.package_root);
        let (dependency_digest, persistent) = match dependency_digest_with(
            &discovery.package_root,
            &discovery.dependencies,
            dependency_file_limit,
            dependency_byte_limit,
        ) {
            Ok(digest) => (digest, true),
            Err(error) => {
                eprintln!(
                    "dotfmt: tailwind: {error}; persistent order cache disabled for {}",
                    discovery.package_root.display()
                );
                (format!("unavailable: {error}"), false)
            }
        };
        let persistent = persistent && !is_tailwind_v3(&discovery.tailwind_version);
        let key = cache_key_for_discovery(&discovery, &dependency_digest);
        let source = format!(
            "project (tailwindcss {}, prettier-plugin-tailwindcss {})",
            discovery.tailwind_version.as_deref().unwrap_or("absent"),
            discovery.plugin_version.as_deref().unwrap_or("absent"),
        );
        Ok(Self {
            root: discovery.package_root.clone(),
            workspace_root: discovery.workspace_root.clone(),
            stylesheet: discovery.stylesheet.clone(),
            config: discovery.config.clone(),
            search_paths,
            key,
            source,
            cache_root: cache_root.to_path_buf(),
            persistent,
            tailwind_version: discovery.tailwind_version.clone(),
            plugin_version: discovery.plugin_version.clone(),
            warnings: discovery.warnings.clone(),
            dependencies: discovery.dependencies.clone(),
        })
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            package_root: self.root.clone(),
            workspace_root: self.workspace_root.clone(),
            stylesheet: self.stylesheet.clone(),
            config: self.config.clone(),
            tailwind_version: self.tailwind_version.clone(),
            plugin_version: self.plugin_version.clone(),
            warnings: self.warnings.clone(),
            dependencies: self.dependencies.clone(),
        }
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

    fn entry_key(&self, candidate_set: &str) -> String {
        format!("{}\n{candidate_set}", self.key)
    }

    fn cache_path(&self, candidate_set: &str) -> PathBuf {
        self.cache_root
            .join(format!("{}.json", hash_key(&self.entry_key(candidate_set))))
    }

    fn load_candidate(&self, candidate_set: &str) -> Option<OrderTable> {
        let key = self.entry_key(candidate_set);
        if let Some(table) = memory_get(&key) {
            return Some(table);
        }
        if !self.persistent {
            return None;
        }
        let cache = read_cache(&self.cache_path(candidate_set), &self.key, candidate_set)?;
        let table = OrderTable::from_pairs(cache.orders).with_source(cache.source);
        memory_put(&key, &table);
        Some(table)
    }

    fn remember_candidate(&self, candidate_set: &str, table: &OrderTable) {
        memory_put(&self.entry_key(candidate_set), table);
    }

    fn persist_candidate(&self, candidate_set: &str, table: &OrderTable) {
        if !self.persistent {
            return;
        }
        let orders = table
            .pairs()
            .map(|(class, order)| (class.to_owned(), order));
        update_cache(
            &self.cache_path(candidate_set),
            &self.key,
            candidate_set,
            &self.source,
            orders,
        );
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
    candidate_set: String,
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

/// Resolve an explicit setting against its declaring config layer. A global
/// setting has no base and resolves against the workspace root.
fn resolve_declared_path(path: &Path, base: Option<&Path>, workspace_root: &Path) -> PathBuf {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.unwrap_or(workspace_root).join(path)
    };
    fs::canonicalize(&resolved).unwrap_or(resolved)
}

fn tailwind_major(version: &Option<String>) -> Option<u32> {
    let version = version.as_deref()?.trim_start_matches('v');
    version.split('.').next()?.parse().ok()
}

fn discovery_cache() -> &'static Mutex<HashMap<String, (Instant, Discovery)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Discovery)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn discovery_key(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
) -> String {
    let start = source.map_or_else(|| absolute(root), absolute);
    let package_root = find_project_root(&start);
    let search_paths = search_paths(&package_root);
    let tailwind_version = package_version("tailwindcss", &package_root, &search_paths);
    let plugin_version =
        package_version("prettier-plugin-tailwindcss", &package_root, &search_paths);
    format!(
        "root={}\nsource={}:{}\nstylesheet={}:{}\nstylesheet_base={}\nconfig={}:{}\nconfig_base={}\ntailwind={}\nplugin={}",
        absolute(root).display(),
        source.map(path_string).unwrap_or_default(),
        file_stamp(source),
        stylesheet.map(path_string).unwrap_or_default(),
        file_stamp(stylesheet),
        stylesheet_base.map(path_string).unwrap_or_default(),
        config.map(path_string).unwrap_or_default(),
        file_stamp(config),
        config_base.map(path_string).unwrap_or_default(),
        tailwind_version.as_deref().unwrap_or("absent"),
        plugin_version.as_deref().unwrap_or("absent"),
    )
}

fn discover_project_cached(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
) -> Result<Discovery, BridgeError> {
    let key = discovery_key(
        root,
        source,
        stylesheet,
        stylesheet_base,
        config,
        config_base,
    );
    {
        let mut cache = discovery_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((inserted, discovery)) = cache.get(&key) {
            if inserted.elapsed() < DISCOVERY_TTL {
                return Ok(discovery.clone());
            }
            cache.remove(&key);
        }
    }
    let discovery = discover_project_uncached(
        root,
        source,
        stylesheet,
        stylesheet_base,
        config,
        config_base,
    )?;
    discovery_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(key, (Instant::now(), discovery.clone()));
    Ok(discovery)
}

fn discover_project_uncached(
    root: &Path,
    source: Option<&Path>,
    stylesheet: Option<&Path>,
    stylesheet_base: Option<&Path>,
    config: Option<&Path>,
    config_base: Option<&Path>,
) -> Result<Discovery, BridgeError> {
    let start = source.map_or_else(|| absolute(root), absolute);
    let package_root = find_project_root(&start);
    let workspace_root = find_workspace_root(&package_root);
    let search_paths = search_paths(&package_root);
    let tailwind_version = package_version("tailwindcss", &package_root, &search_paths);
    let plugin_version =
        package_version("prettier-plugin-tailwindcss", &package_root, &search_paths);
    let detected_major = tailwind_major(&tailwind_version);
    // A project with a config but no readable tailwind package is treated as
    // v3 so config-only projects keep working through the plugin.
    let major = detected_major.or_else(|| {
        find_config_between(&package_root, &workspace_root)
            .is_some()
            .then_some(3)
    });
    let explicit_stylesheet =
        stylesheet.map(|path| resolve_declared_path(path, stylesheet_base, &workspace_root));
    let explicit_config =
        config.map(|path| resolve_declared_path(path, config_base, &workspace_root));
    if let Some(path) = &explicit_stylesheet
        && !path.is_file()
    {
        return Err(BridgeError::MissingStylesheet(path.clone()));
    }
    if let Some(path) = &explicit_config
        && !path.is_file()
    {
        return Err(BridgeError::MissingConfig(path.clone()));
    }
    let mut warnings = Vec::new();

    let (stylesheet, config) = if major == Some(3) {
        if explicit_stylesheet.is_some() && explicit_config.is_some() {
            warnings.push(
                "both tailwind_stylesheet and tailwind_config are set; Tailwind v3 uses tailwind_config"
                    .to_owned(),
            );
        } else if explicit_stylesheet.is_some() {
            warnings.push(
                "tailwind_stylesheet is ignored for Tailwind v3; using tailwind_config".to_owned(),
            );
        }
        let config =
            explicit_config.or_else(|| find_config_between(&package_root, &workspace_root));
        (None, config)
    } else {
        if explicit_stylesheet.is_some() && explicit_config.is_some() {
            warnings.push(
                "both tailwind_stylesheet and tailwind_config are set; Tailwind v4 uses tailwind_stylesheet"
                    .to_owned(),
            );
        } else if explicit_config.is_some() {
            warnings.push(
                "tailwind_config is ignored for Tailwind v4; using the discovered stylesheet"
                    .to_owned(),
            );
        }
        let stylesheet = match explicit_stylesheet {
            Some(path) => Some(path),
            None => discover_stylesheet(source, &package_root, &workspace_root)?,
        };
        (stylesheet, None)
    };

    let dependencies = css_dependencies(stylesheet.as_deref(), source, &package_root);
    Ok(Discovery {
        package_root,
        workspace_root,
        stylesheet,
        config,
        tailwind_version,
        plugin_version,
        warnings,
        dependencies,
    })
}

fn discover_stylesheet(
    source: Option<&Path>,
    package_root: &Path,
    workspace_root: &Path,
) -> Result<Option<PathBuf>, BridgeError> {
    let mut candidates: Vec<(u8, PathBuf)> = Vec::new();
    if let Some(source) = source {
        let source = absolute(source);
        if is_css(&source) && is_tailwind_stylesheet(&source) {
            candidates.push((2, source.clone()));
        }
        for imported in source_css_imports(&source, package_root) {
            candidates.push((4, imported));
        }
    }
    for path in package_styles(package_root) {
        candidates.push((3, path));
    }
    for name in CONVENTIONAL_STYLESHEETS {
        let path = package_root.join(name);
        if path.is_file() && is_tailwind_stylesheet(&path) {
            candidates.push((5, path));
        }
    }
    if package_root == workspace_root
        && let Ok(entries) = fs::read_dir(workspace_root)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_css(&path) && is_tailwind_stylesheet(&path) {
                candidates.push((6, path));
            }
        }
    }

    let mut best: BTreeMap<PathBuf, u8> = BTreeMap::new();
    for (score, path) in candidates {
        let Some(path) = existing_path(&path) else {
            continue;
        };
        best.entry(path)
            .and_modify(|current| *current = (*current).min(score))
            .or_insert(score);
    }
    let Some(top_score) = best.values().copied().min() else {
        return Ok(None);
    };
    let mut top: Vec<PathBuf> = best
        .into_iter()
        .filter_map(|(path, score)| (score == top_score).then_some(path))
        .collect();
    top.sort();
    match top.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err(BridgeError::AmbiguousStylesheet(top)),
    }
}

fn existing_path(path: &Path) -> Option<PathBuf> {
    let path = absolute(path);
    path.is_file()
        .then(|| fs::canonicalize(&path).unwrap_or(path))
}

fn is_css(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
}

fn is_tailwind_stylesheet(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    text.contains("@import \"tailwindcss")
        || text.contains("@import 'tailwindcss")
        || text.contains("@tailwind")
        || text.contains("@theme")
        || text.contains("@utility")
        || text.contains("@plugin")
}

fn package_styles(package_root: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(package_root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    if let Some(style) = package.get("style").and_then(serde_json::Value::as_str) {
        entries.push(style.to_owned());
    }
    if let Some(main) = package.get("main").and_then(serde_json::Value::as_str)
        && main.ends_with(".css")
    {
        entries.push(main.to_owned());
    }
    if let Some(exports) = package.get("exports") {
        collect_css_exports(exports, &mut entries);
    }
    entries
        .into_iter()
        .map(|entry| package_root.join(entry.trim_start_matches("./")))
        .filter(|path| path.is_file())
        .collect()
}

fn collect_css_exports(value: &serde_json::Value, entries: &mut Vec<String>) {
    match value {
        serde_json::Value::String(entry) if entry.ends_with(".css") => {
            entries.push(entry.clone());
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_css_exports(value, entries);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_css_exports(value, entries);
            }
        }
        _ => {}
    }
}

fn source_css_imports(source: &Path, package_root: &Path) -> Vec<PathBuf> {
    let mut dependencies = Vec::new();
    let mut seen = HashSet::new();
    collect_imported_css(source, package_root, &mut seen, &mut dependencies);
    dependencies
}

fn css_dependencies(
    stylesheet: Option<&Path>,
    source: Option<&Path>,
    package_root: &Path,
) -> Vec<PathBuf> {
    let mut dependencies = Vec::new();
    let mut seen = HashSet::new();
    if let Some(stylesheet) = stylesheet {
        collect_imported_css(stylesheet, package_root, &mut seen, &mut dependencies);
        if let Some(path) = existing_path(stylesheet) {
            dependencies.push(path);
        }
    }
    if let Some(source) = source {
        collect_imported_css(
            &absolute(source),
            package_root,
            &mut seen,
            &mut dependencies,
        );
    }
    dependencies.sort();
    dependencies.dedup();
    dependencies
}

fn collect_imported_css(
    origin: &Path,
    package_root: &Path,
    seen: &mut HashSet<PathBuf>,
    dependencies: &mut Vec<PathBuf>,
) {
    let origin = absolute(origin);
    if !seen.insert(origin.clone()) {
        return;
    }
    let Ok(text) = fs::read_to_string(&origin) else {
        return;
    };
    for specifier in quoted_literals(&text) {
        if !specifier.ends_with(".css") {
            continue;
        }
        let Some(path) = resolve_import(&origin, &specifier, package_root) else {
            continue;
        };
        if seen.insert(path.clone()) {
            dependencies.push(path.clone());
            collect_imported_css(&path, package_root, seen, dependencies);
        }
    }
}

fn resolve_import(origin: &Path, specifier: &str, package_root: &Path) -> Option<PathBuf> {
    let path = if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
        origin.parent().unwrap_or(origin).join(specifier)
    } else {
        package_root.join("node_modules").join(specifier)
    };
    existing_path(&path)
}

fn quoted_literals(text: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;
    for (offset, character) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        match quote {
            Some(current) if character == current => {
                literals.push(text[start..offset].to_owned());
                quote = None;
            }
            Some(_) => {}
            None if character == '\'' || character == '"' => {
                quote = Some(character);
                start = offset + character.len_utf8();
            }
            None => {}
        }
    }
    literals
}

pub fn find_workspace_root(start: &Path) -> PathBuf {
    let start = absolute(start);
    let mut current = if start.is_file() {
        start.parent().unwrap_or(&start).to_path_buf()
    } else {
        start.clone()
    };
    loop {
        if is_workspace_marker(&current) {
            return fs::canonicalize(&current).unwrap_or(current);
        }
        if !current.pop() {
            break;
        }
    }
    find_project_root(&start)
}

fn is_workspace_marker(directory: &Path) -> bool {
    if directory.join(".git").exists()
        || directory.join("pnpm-workspace.yaml").is_file()
        || directory.join("lerna.json").is_file()
    {
        return true;
    }
    let Ok(text) = fs::read_to_string(directory.join("package.json")) else {
        return false;
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    package.get("workspaces").is_some()
}

fn find_config_between(package_root: &Path, workspace_root: &Path) -> Option<PathBuf> {
    let mut current = Some(package_root);
    while let Some(directory) = current {
        for name in TAILWIND_CONFIG_NAMES {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if directory == workspace_root {
            break;
        }
        current = directory.parent();
    }
    None
}

fn find_config(root: &Path) -> Option<PathBuf> {
    let mut current = Some(root);
    while let Some(directory) = current {
        for name in TAILWIND_CONFIG_NAMES {
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

#[cfg(test)]
fn cache_key(
    root: &Path,
    stylesheet: Option<&Path>,
    config: Option<&Path>,
    tailwind_version: &Option<String>,
    plugin_version: &Option<String>,
    dependency_digest: &str,
) -> String {
    format!(
        "version={CACHE_VERSION}\nroot={}\nstylesheet={}:{}\nconfig={}:{}\ntailwind={}\nplugin={}\ndependencies={}",
        root.display(),
        stylesheet.map(path_string).unwrap_or_default(),
        file_stamp(stylesheet),
        config.map(path_string).unwrap_or_default(),
        file_stamp(config),
        tailwind_version.as_deref().unwrap_or("absent"),
        plugin_version.as_deref().unwrap_or("absent"),
        dependency_digest,
    )
}

fn cache_key_for_discovery(discovery: &Discovery, dependency_digest: &str) -> String {
    format!(
        "version={CACHE_VERSION}\nworkspace={}\nroot={}\nstylesheet={}:{}\nconfig={}:{}\ntailwind={}\nplugin={}\ndependencies={}",
        discovery.workspace_root.display(),
        discovery.package_root.display(),
        discovery
            .stylesheet
            .as_deref()
            .map(path_string)
            .unwrap_or_default(),
        file_stamp(discovery.stylesheet.as_deref()),
        discovery
            .config
            .as_deref()
            .map(path_string)
            .unwrap_or_default(),
        file_stamp(discovery.config.as_deref()),
        discovery.tailwind_version.as_deref().unwrap_or("absent"),
        discovery.plugin_version.as_deref().unwrap_or("absent"),
        dependency_digest,
    )
}

fn is_tailwind_v3(version: &Option<String>) -> bool {
    tailwind_major(version) == Some(3)
}

#[derive(Debug)]
enum DependencyScanError {
    Entries(usize),
    Files(usize),
    Bytes(u64),
}

impl std::fmt::Display for DependencyScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entries(limit) => write!(formatter, "dependency scan exceeded {limit} entries"),
            Self::Files(limit) => write!(formatter, "dependency scan exceeded {limit} files"),
            Self::Bytes(limit) => write!(
                formatter,
                "dependency scan exceeded {} MiB",
                limit / (1024 * 1024)
            ),
        }
    }
}

#[cfg(test)]
fn dependency_digest(
    root: &Path,
    file_limit: usize,
    byte_limit: u64,
) -> Result<String, DependencyScanError> {
    dependency_digest_with(root, &[], file_limit, byte_limit)
}

fn dependency_digest_with(
    root: &Path,
    extra_paths: &[PathBuf],
    file_limit: usize,
    byte_limit: u64,
) -> Result<String, DependencyScanError> {
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .filter_entry(is_dependency_entry)
        .build();
    let mut paths = Vec::new();
    for (index, entry) in walker.flatten().enumerate() {
        if index >= DEPENDENCY_ENTRY_LIMIT {
            return Err(DependencyScanError::Entries(DEPENDENCY_ENTRY_LIMIT));
        }
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || !is_dependency_source(path) {
            continue;
        }
        paths.push(path.to_path_buf());
        if paths.len() > file_limit {
            return Err(DependencyScanError::Files(file_limit));
        }
    }
    for path in extra_paths {
        if path.is_file() {
            paths.push(path.clone());
            if paths.len() > file_limit {
                return Err(DependencyScanError::Files(file_limit));
            }
        }
    }
    for name in DEPENDENCY_LOCKFILES {
        let path = root.join(name);
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    if paths.len() > file_limit {
        return Err(DependencyScanError::Files(file_limit));
    }

    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for path in paths {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        total = total.saturating_add(metadata.len());
        if total > byte_limit {
            return Err(DependencyScanError::Bytes(byte_limit));
        }
        hasher.update(
            path.strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .as_bytes(),
        );
        hasher.update([0]);
        match fs::read(&path) {
            Ok(bytes) => hasher.update(bytes),
            Err(error) => {
                hasher.update(b"unreadable:");
                hasher.update(error.kind().to_string().as_bytes());
            }
        }
        hasher.update([0]);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn is_dependency_entry(entry: &ignore::DirEntry) -> bool {
    entry.depth() == 0
        || !entry
            .file_name()
            .to_str()
            .is_some_and(|name| matches!(name, "node_modules" | "dist" | "build" | ".git"))
}

fn is_dependency_source(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
        || path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| TAILWIND_CONFIG_NAMES.contains(&name))
}

fn file_stamp(path: Option<&Path>) -> u128 {
    path.and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let mut hash = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(hash, "{byte:02x}");
    }
    hash
}

fn hash_key(key: &str) -> String {
    hex_digest(Sha256::digest(key.as_bytes()))
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
    cache.insert(key.to_owned(), table.clone());
}

fn read_cache(path: &Path, key: &str, candidate_set: &str) -> Option<DiskCache> {
    let text = fs::read_to_string(path).ok()?;
    let cache: DiskCache = serde_json::from_str(&text).ok()?;
    (cache.version == CACHE_VERSION && cache.key == key && cache.candidate_set == candidate_set)
        .then_some(cache)
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
    candidate_set: &str,
    source: &str,
    orders: impl IntoIterator<Item = (String, Option<i64>)>,
) {
    let _lock = CacheLock::acquire(path);
    let cache = DiskCache {
        version: CACHE_VERSION,
        key: key.to_owned(),
        candidate_set: candidate_set.to_owned(),
        source: source.to_owned(),
        orders: orders.into_iter().collect(),
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

    const FAKE_ORDER: &[&str] = &["m-2", "flex", "p-2", "p-4", "text-sm", "font-bold", "gap-2"];

    fn fake_response_with_order(classes: &[&str], order: &[&str]) -> HelperResponse {
        let known: Vec<&str> = order
            .iter()
            .copied()
            .filter(|class| classes.contains(class))
            .collect();
        HelperResponse {
            ok: true,
            error: None,
            message: None,
            orders: classes
                .iter()
                .map(|class| {
                    (
                        (*class).to_owned(),
                        known
                            .iter()
                            .position(|known| known == class)
                            .map(|index| index as i64),
                    )
                })
                .collect(),
            classes: None,
        }
    }

    fn fake_response(classes: &[&str]) -> HelperResponse {
        fake_response_with_order(classes, FAKE_ORDER)
    }

    fn run_fake(
        root: &Path,
        classes: &[String],
        cache_root: &Path,
        queries: Option<&Mutex<Vec<Vec<String>>>>,
    ) -> OrderTable {
        let query = |_: &Project, requested: Vec<&str>| {
            if let Some(queries) = queries {
                queries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(requested.iter().map(|class| (*class).to_owned()).collect());
            }
            Ok(fake_response(&requested))
        };
        project_order_with_cache_using(root, None, None, classes, cache_root, query)
            .expect("fake project order")
            .table
    }

    fn write_package(project: &Path, name: &str, version: &str) {
        let directory = project.join("node_modules").join(name);
        fs::create_dir_all(&directory).expect("package directory");
        fs::write(
            directory.join("package.json"),
            format!("{{\"name\":\"{name}\",\"version\":\"{version}\"}}\n"),
        )
        .expect("package.json");
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
    fn cache_updates_replace_entries_instead_of_merging() {
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join("cache.json");
        let key = "theme-key";
        let candidate_set = "candidate-set";
        update_cache(
            &path,
            key,
            candidate_set,
            "test",
            [("p-2".to_owned(), Some(1))],
        );
        update_cache(
            &path,
            key,
            candidate_set,
            "test",
            [("p-4".to_owned(), Some(2))],
        );
        let cache = read_cache(&path, key, candidate_set).expect("cache");
        assert_eq!(cache.orders.len(), 1);
        assert_eq!(cache.orders.get("p-4"), Some(&Some(2)));
        assert!(!cache.orders.contains_key("p-2"));
        assert!(read_cache(&path, key, "other-set").is_none());
    }

    #[test]
    fn concurrent_cache_updates_leave_one_complete_entry() {
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join("cache.json");
        let key = "concurrent-key";
        let candidate_set = "concurrent-set";
        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    update_cache(
                        path,
                        key,
                        candidate_set,
                        "test",
                        [(format!("class-{index}"), Some(index as i64))],
                    );
                });
            }
        });
        let cache = read_cache(&path, key, candidate_set).expect("cache");
        assert_eq!(cache.orders.len(), 1);
        assert!(cache.orders.keys().all(|class| class.starts_with("class-")));
    }

    #[test]
    fn memory_updates_replace_entries() {
        let key = "memory-replace-test";
        memory_put(key, &OrderTable::from_pairs([("p-2".to_owned(), Some(1))]));
        memory_put(key, &OrderTable::from_pairs([("p-4".to_owned(), Some(2))]));
        let table = memory_get(key).expect("memory table");
        assert_eq!(table.len(), 1);
        assert_eq!(table.order("p-4"), Some(Some(2)));
        assert_eq!(table.order("p-2"), None);
        memory_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }

    #[test]
    fn cache_key_tracks_stylesheet_mtime_and_versions() {
        let project = empty_project();
        let stylesheet = project.path().join("app.css");
        fs::write(&stylesheet, "a {}\n").expect("stylesheet");
        let root = project.path();
        let tailwind = Some("4.0.0".to_owned());
        let plugin = Some("0.8.0".to_owned());
        let first = cache_key(root, Some(&stylesheet), None, &tailwind, &plugin, "deps-a");
        std::thread::sleep(Duration::from_millis(5));
        fs::write(&stylesheet, "b {}\n").expect("stylesheet");
        let second = cache_key(root, Some(&stylesheet), None, &tailwind, &plugin, "deps-a");
        assert_ne!(first, second);
        let other_tailwind = Some("4.1.0".to_owned());
        assert_ne!(
            second,
            cache_key(
                root,
                Some(&stylesheet),
                None,
                &other_tailwind,
                &plugin,
                "deps-a"
            )
        );
        let other_plugin = Some("0.9.0".to_owned());
        assert_ne!(
            second,
            cache_key(
                root,
                Some(&stylesheet),
                None,
                &tailwind,
                &other_plugin,
                "deps-a"
            )
        );
        assert_ne!(
            second,
            cache_key(root, Some(&stylesheet), None, &tailwind, &plugin, "deps-b")
        );
    }

    #[test]
    fn dependency_digest_tracks_css_and_config_content_and_caps() {
        let project = empty_project();
        let root = project.path();
        fs::write(root.join("theme.css"), ".a {}\n").expect("css");
        fs::write(root.join("tailwind.config.js"), "module.exports = {};\n").expect("config");
        let first = dependency_digest(root, 10, 1024).expect("digest");
        for directory in ["node_modules", "dist", "build", ".git"] {
            let directory = root.join(directory);
            fs::create_dir_all(&directory).expect("ignored directory");
            fs::write(directory.join("ignored.css"), ".ignored {}\n").expect("ignored css");
        }
        assert_eq!(dependency_digest(root, 10, 1024).expect("digest"), first);
        fs::write(root.join("package-lock.json"), "{}\n").expect("lockfile");
        let with_lockfile = dependency_digest(root, 10, 1024).expect("digest");
        assert_ne!(first, with_lockfile);
        fs::write(root.join("theme.css"), ".b {}\n").expect("css");
        let second = dependency_digest(root, 10, 1024).expect("digest");
        assert_ne!(with_lockfile, second);
        fs::write(
            root.join("tailwind.config.js"),
            "module.exports = { darkMode: 'class' };\n",
        )
        .expect("config");
        let third = dependency_digest(root, 10, 1024).expect("digest");
        assert_ne!(second, third);
        assert!(matches!(
            dependency_digest(root, 1, 1024),
            Err(DependencyScanError::Files(1))
        ));
        assert!(matches!(
            dependency_digest(root, 10, 1),
            Err(DependencyScanError::Bytes(1))
        ));
    }

    #[test]
    fn cache_version_migration_invalidates_entries() {
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join("cache.json");
        let key = "theme-key";
        let candidate_set = "candidate-set";
        let old = DiskCache {
            version: CACHE_VERSION - 1,
            key: key.to_owned(),
            candidate_set: candidate_set.to_owned(),
            source: "test".to_owned(),
            orders: BTreeMap::from([("p-2".to_owned(), Some(1))]),
        };
        write_cache(&path, &old);
        assert!(read_cache(&path, key, candidate_set).is_none());
        let current = DiskCache {
            version: CACHE_VERSION,
            ..old
        };
        write_cache(&path, &current);
        assert!(read_cache(&path, key, candidate_set).is_some());
    }

    #[test]
    fn seed_then_target_queries_the_full_candidate_set() {
        let project = empty_project();
        let cache = tempfile::tempdir().expect("cache directory");
        let queries = Mutex::new(Vec::new());
        let seed = run_fake(
            project.path(),
            &["p-2".to_owned(), "text-sm".to_owned()],
            cache.path(),
            Some(&queries),
        );
        assert!(
            seed.order("p-2").flatten() < seed.order("text-sm").flatten(),
            "{seed:?}"
        );

        let target_classes = vec!["p-4".to_owned(), "p-2".to_owned(), "text-sm".to_owned()];
        let target = run_fake(
            project.path(),
            &target_classes,
            cache.path(),
            Some(&queries),
        );
        assert!(
            target.order("p-2").flatten() < target.order("p-4").flatten(),
            "{target:?}"
        );
        assert_eq!(
            crate::sorter::sort_class_list("p-4 p-2 text-sm", &target, true),
            "p-2 p-4 text-sm"
        );
        let queries = queries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[1], target_classes);
    }

    #[test]
    fn a_b_c_batches_do_not_depend_on_file_history() {
        let project = empty_project();
        let cache = tempfile::tempdir().expect("cache directory");
        let queries = Mutex::new(Vec::new());
        let a_classes = vec!["p-4".to_owned(), "p-2".to_owned()];
        let b_classes = vec!["p-2".to_owned(), "text-sm".to_owned()];
        let c_classes = vec!["p-4".to_owned(), "text-sm".to_owned()];
        let a = run_fake(project.path(), &a_classes, cache.path(), Some(&queries));
        let b = run_fake(project.path(), &b_classes, cache.path(), Some(&queries));
        let c = run_fake(project.path(), &c_classes, cache.path(), Some(&queries));

        assert_eq!(
            crate::sorter::sort_class_list("p-4 p-2", &a, true),
            "p-2 p-4"
        );
        assert_eq!(
            crate::sorter::sort_class_list("p-2 text-sm", &b, true),
            "p-2 text-sm"
        );
        assert_eq!(
            crate::sorter::sort_class_list("p-4 text-sm", &c, true),
            "p-4 text-sm"
        );
        let queries = queries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(queries.len(), 3);
        assert_eq!(queries[0], a_classes);
        assert_eq!(queries[1], b_classes);
        assert_eq!(queries[2], c_classes);
    }

    #[test]
    fn warm_cache_matches_a_clean_cache() {
        let project = empty_project();
        let warm_cache = tempfile::tempdir().expect("warm cache");
        let clean_cache = tempfile::tempdir().expect("clean cache");
        let target_classes = vec!["p-4".to_owned(), "p-2".to_owned(), "text-sm".to_owned()];
        run_fake(
            project.path(),
            &["p-2".to_owned(), "text-sm".to_owned()],
            warm_cache.path(),
            None,
        );
        let warm = run_fake(project.path(), &target_classes, warm_cache.path(), None);
        let warm_project = Project::discover(
            project.path(),
            None,
            None,
            None,
            None,
            None,
            warm_cache.path(),
        )
        .expect("warm project");
        let target_refs: Vec<&str> = target_classes.iter().map(String::as_str).collect();
        let target_key = warm_project.entry_key(&candidate_set_hash(&target_refs));
        memory_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&target_key);
        let clean = run_fake(project.path(), &target_classes, clean_cache.path(), None);
        assert_eq!(
            crate::sorter::sort_class_list("p-4 p-2 text-sm", &warm, true),
            crate::sorter::sort_class_list("p-4 p-2 text-sm", &clean, true)
        );
    }

    #[test]
    fn repeat_pass_reuses_exact_candidate_set_and_is_idempotent() {
        let project = empty_project();
        let cache = tempfile::tempdir().expect("cache directory");
        let queries = Mutex::new(Vec::new());
        let classes = vec!["p-4".to_owned(), "p-2".to_owned(), "text-sm".to_owned()];
        let first = run_fake(project.path(), &classes, cache.path(), Some(&queries));
        let once = crate::sorter::sort_class_list("p-4 p-2 text-sm", &first, true);
        let second = run_fake(project.path(), &classes, cache.path(), Some(&queries));
        let twice = crate::sorter::sort_class_list(&once, &second, true);
        assert_eq!(once, twice);
        assert_eq!(crate::sorter::sort_class_list(&twice, &second, true), twice);
        assert_eq!(
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );
    }

    #[test]
    fn imported_theme_edit_invalidates_the_order_cache() {
        let project = empty_project();
        let cache = tempfile::tempdir().expect("cache directory");
        fs::write(project.path().join("app.css"), "@import \"./theme.css\";\n")
            .expect("stylesheet");
        let theme = project.path().join("theme.css");
        fs::write(&theme, "@theme { --order: p2-first; }\n").expect("theme");
        let queries = Mutex::new(Vec::<Vec<String>>::new());
        let query = |project: &Project, requested: Vec<&str>| {
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(requested.iter().map(|class| (*class).to_owned()).collect());
            let theme = fs::read_to_string(project.root.join("theme.css")).expect("theme");
            let order: &[&str] = if theme.contains("p2-first") {
                &["p-2", "p-4"]
            } else {
                &["p-4", "p-2"]
            };
            Ok(fake_response_with_order(&requested, order))
        };
        let classes = vec!["p-4".to_owned(), "p-2".to_owned()];
        let first = project_order_with_cache_using(
            project.path(),
            Some(Path::new("app.css")),
            None,
            &classes,
            cache.path(),
            query,
        )
        .expect("first order")
        .table;
        assert!(first.order("p-2").flatten() < first.order("p-4").flatten());

        fs::write(&theme, "@theme { --order: p4-first; }\n").expect("theme edit");
        let second = project_order_with_cache_using(
            project.path(),
            Some(Path::new("app.css")),
            None,
            &classes,
            cache.path(),
            query,
        )
        .expect("second order")
        .table;
        assert!(second.order("p-4").flatten() < second.order("p-2").flatten());
        assert_eq!(
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            2
        );
    }

    #[test]
    fn plugin_and_tailwind_version_bumps_invalidate_the_theme() {
        let project = empty_project();
        let cache = tempfile::tempdir().expect("cache directory");
        let queries = Mutex::new(Vec::new());
        let classes = vec!["p-4".to_owned(), "p-2".to_owned()];
        write_package(project.path(), "tailwindcss", "4.0.0");
        write_package(project.path(), "prettier-plugin-tailwindcss", "0.8.0");
        run_fake(project.path(), &classes, cache.path(), Some(&queries));
        run_fake(project.path(), &classes, cache.path(), Some(&queries));
        assert_eq!(
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );

        write_package(project.path(), "tailwindcss", "4.1.0");
        run_fake(project.path(), &classes, cache.path(), Some(&queries));
        assert_eq!(
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            2
        );

        write_package(project.path(), "prettier-plugin-tailwindcss", "0.9.0");
        run_fake(project.path(), &classes, cache.path(), Some(&queries));
        assert_eq!(
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            3
        );
    }

    #[test]
    fn v3_disables_persistent_order_caching() {
        let project = empty_project();
        write_package(project.path(), "tailwindcss", "3.4.0");
        let cache = tempfile::tempdir().expect("cache directory");
        let classes = vec!["p-4".to_owned(), "p-2".to_owned()];
        run_fake(project.path(), &classes, cache.path(), None);
        run_fake(project.path(), &classes, cache.path(), None);
        assert_eq!(
            fs::read_dir(cache.path()).expect("cache directory").count(),
            0
        );
    }

    #[test]
    fn dependency_cap_disables_persistence() {
        let directory = empty_project();
        fs::write(directory.path().join("a.css"), ".a {}\n").expect("css");
        fs::write(directory.path().join("b.css"), ".b {}\n").expect("css");
        let cache = tempfile::tempdir().expect("cache directory");
        let project = Project::discover_with_limits(
            directory.path(),
            None,
            None,
            None,
            None,
            None,
            cache.path(),
            1,
            DEPENDENCY_BYTE_LIMIT,
        )
        .expect("project discovery");
        assert!(!project.persistent);
        let queries = Mutex::new(Vec::<Vec<String>>::new());
        let query = |_: &Project, requested: Vec<&str>| {
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(requested.iter().map(|class| (*class).to_owned()).collect());
            Ok(fake_response(&requested))
        };
        let classes = vec!["p-4".to_owned(), "p-2".to_owned()];
        project_order_with_project(project, &classes, query).expect("first order");
        assert_eq!(
            fs::read_dir(cache.path()).expect("cache directory").count(),
            0
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
        let cached = project_order_with_cache(
            project.path(),
            Some(Path::new("app.css")),
            None,
            &["gap-2".to_owned(), "custom".to_owned()],
            cache.path(),
        )
        .expect("cached order");
        assert_eq!(cached.order("m-2"), None);
        assert_eq!(cached.order("custom"), Some(None));
        assert!(cached.order("gap-2").flatten().is_some());
        assert_eq!(cached.len(), 2);
        let disks: Vec<DiskCache> = fs::read_dir(cache.path())
            .expect("cache directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .map(|path| {
                serde_json::from_str(&fs::read_to_string(path).expect("cache contents"))
                    .expect("cache JSON")
            })
            .collect();
        assert!(disks.iter().any(|disk| disk.orders.contains_key("m-2")));
        let cached_disk = disks
            .iter()
            .find(|disk| disk.orders.contains_key("gap-2"))
            .expect("cached candidate set");
        assert_eq!(cached_disk.orders.len(), 2);
        assert!(!cached_disk.orders.contains_key("m-2"));
    }

    #[test]
    fn fake_node_plugin_receives_the_full_candidate_set_on_cache_miss() {
        if !node_available() {
            return;
        }
        let project = empty_project();
        let package = project
            .path()
            .join("node_modules/prettier-plugin-tailwindcss");
        fs::create_dir_all(&package).expect("plugin package");
        fs::write(
            package.join("package.json"),
            r#"{"name":"prettier-plugin-tailwindcss","version":"0.8.0","type":"module","exports":{".":"./index.js","./sorter":"./sorter.js"}}"#,
        )
        .expect("package.json");
        fs::write(
            package.join("index.js"),
            "export { getTailwindConfig } from './sorter.js';\n",
        )
        .expect("index.js");
        fs::write(
            package.join("sorter.js"),
            r#"const ORDER = ["p-2", "p-4", "text-sm"];
export async function getTailwindConfig() {
  return {
    getClassOrder(classes) {
      const known = ORDER.filter((name) => classes.includes(name));
      return classes.map((name) => [name, known.includes(name) ? known.indexOf(name) : null]);
    },
  };
}
"#,
        )
        .expect("sorter.js");
        let cache = tempfile::tempdir().expect("cache directory");
        project_order_with_cache(
            project.path(),
            None,
            None,
            &["p-2".to_owned(), "text-sm".to_owned()],
            cache.path(),
        )
        .expect("seed order");
        let target = project_order_with_cache(
            project.path(),
            None,
            None,
            &["p-4".to_owned(), "p-2".to_owned(), "text-sm".to_owned()],
            cache.path(),
        )
        .expect("target order");
        assert_eq!(
            crate::sorter::sort_class_list("p-4 p-2 text-sm", &target, true),
            "p-2 p-4 text-sm"
        );
    }

    fn discover(
        root: &Path,
        source: &Path,
        stylesheet: Option<&Path>,
        stylesheet_base: Option<&Path>,
        config: Option<&Path>,
        config_base: Option<&Path>,
    ) -> Result<Discovery, BridgeError> {
        super::discover_project(
            root,
            Some(source),
            stylesheet,
            stylesheet_base,
            config,
            config_base,
        )
    }

    fn clear_discovery(
        root: &Path,
        source: &Path,
        stylesheet: Option<&Path>,
        stylesheet_base: Option<&Path>,
        config: Option<&Path>,
        config_base: Option<&Path>,
    ) {
        discovery_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&discovery_key(
                root,
                Some(source),
                stylesheet,
                stylesheet_base,
                config,
                config_base,
            ));
    }

    fn write_workspace_root(root: &Path) {
        fs::write(
            root.join("package.json"),
            "{\"name\":\"workspace\",\"workspaces\":[\"packages/*\"]}\n",
        )
        .expect("workspace package.json");
    }

    fn write_package_manifest(root: &Path, name: &str) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).expect("package directory");
        fs::write(
            directory.join("package.json"),
            format!("{{\"name\":\"{name}\"}}\n"),
        )
        .expect("package.json");
    }

    #[test]
    fn v3_config_only_never_passes_a_stylesheet() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "3.4.0");
        fs::write(root.join("tailwind.config.js"), "module.exports = {};\n").expect("config");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(
            root.join("src/app.css"),
            "@import \"tailwindcss\";\n@tailwind base;\n",
        )
        .expect("stylesheet");
        fs::write(root.join("src/a.ts"), "export const a = 1;\n").expect("source");
        let discovery =
            discover(root, &root.join("src/a.ts"), None, None, None, None).expect("v3 discovery");
        assert_eq!(discovery.tailwind_version.as_deref(), Some("3.4.0"));
        assert!(discovery.stylesheet.is_none(), "{discovery:?}");
        assert_eq!(
            discovery.config.as_deref(),
            Some(root.join("tailwind.config.js").as_path())
        );
    }

    #[test]
    fn both_path_settings_choose_by_tailwind_major_with_a_warning() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "4.0.0");
        fs::write(root.join("app.css"), "@import \"tailwindcss\";\n").expect("stylesheet");
        fs::write(root.join("tailwind.config.js"), "module.exports = {};\n").expect("config");
        fs::create_dir_all(root.join("src")).expect("src");
        let source = root.join("src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        let discovery = discover(
            root,
            &source,
            Some(Path::new("app.css")),
            Some(root),
            Some(Path::new("tailwind.config.js")),
            Some(root),
        )
        .expect("v4 discovery");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("app.css").as_path())
        );
        assert!(discovery.config.is_none());
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("Tailwind v4")),
            "{discovery:?}"
        );

        write_package(root, "tailwindcss", "3.4.0");
        let discovery = discover(
            root,
            &source,
            Some(Path::new("app.css")),
            Some(root),
            Some(Path::new("tailwind.config.js")),
            Some(root),
        )
        .expect("v3 discovery");
        assert!(discovery.stylesheet.is_none());
        assert_eq!(
            discovery.config.as_deref(),
            Some(root.join("tailwind.config.js").as_path())
        );
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("Tailwind v3")),
            "{discovery:?}"
        );
    }

    #[test]
    fn two_conventional_stylesheets_in_one_package_are_ambiguous() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "4.0.0");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/index.css"), "@import \"tailwindcss\";\n").expect("index css");
        fs::write(root.join("src/app.css"), "@import \"tailwindcss\";\n").expect("app css");
        let source = root.join("src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        let error = discover(root, &source, None, None, None, None).expect_err("ambiguous");
        let BridgeError::AmbiguousStylesheet(candidates) = error else {
            panic!("unexpected error: {error:?}");
        };
        assert_eq!(candidates.len(), 2, "{candidates:?}");
        assert!(candidates.iter().all(|path| path.starts_with(root)));
    }

    #[test]
    fn package_styles_beat_shared_root_css_and_never_use_siblings() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path();
        write_workspace_root(root);
        write_package(root, "tailwindcss", "4.0.0");
        write_package_manifest(root, "packages/ui");
        fs::create_dir_all(root.join("packages/ui/src")).expect("ui src");
        fs::write(
            root.join("packages/ui/src/ui.css"),
            "@import \"tailwindcss\";\n",
        )
        .expect("ui css");
        fs::write(
            root.join("packages/ui/package.json"),
            "{\"name\":\"ui\",\"style\":\"src/ui.css\"}\n",
        )
        .expect("ui package.json");
        fs::write(root.join("globals.css"), "@import \"tailwindcss\";\n").expect("root css");
        let source = root.join("packages/ui/src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        let discovery = discover(root, &source, None, None, None, None).expect("discovery");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("packages/ui/src/ui.css").as_path())
        );
        assert_eq!(discovery.workspace_root, root);

        write_package_manifest(root, "packages/b");
        fs::create_dir_all(root.join("packages/b/src")).expect("b src");
        fs::write(
            root.join("packages/b/src/app.css"),
            "@import \"tailwindcss\";\n",
        )
        .expect("sibling css");
        let bare_source = root.join("packages/ui/src/bare.tsx");
        fs::write(&bare_source, "export const a = 1;\n").expect("source");
        let discovery =
            discover(root, &bare_source, None, None, None, None).expect("sibling discovery");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("packages/ui/src/ui.css").as_path()),
            "the package style still wins, and the sibling is never scanned"
        );
    }

    #[test]
    fn package_css_exports_are_discovered() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "4.0.0");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/theme.css"), "@import \"tailwindcss\";\n").expect("theme");
        fs::write(
            root.join("package.json"),
            "{\"name\":\"fixture\",\"exports\":{\"./theme\":\"./src/theme.css\"}}\n",
        )
        .expect("package.json");
        let source = root.join("src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        let discovery = discover(root, &source, None, None, None, None).expect("discovery");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("src/theme.css").as_path())
        );
    }

    #[test]
    fn imported_workspace_css_is_used_and_hashed_as_a_dependency() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path();
        write_workspace_root(root);
        write_package(root, "tailwindcss", "4.0.0");
        write_package_manifest(root, "packages/app");
        fs::create_dir_all(root.join("packages/app/src")).expect("src");
        fs::write(
            root.join("packages/app/src/app.css"),
            "@import \"../../../theme.css\";\n",
        )
        .expect("app css");
        fs::write(
            root.join("theme.css"),
            "@import \"tailwindcss\";\n@theme { --color-brand: red; }\n",
        )
        .expect("shared theme");
        let source = root.join("packages/app/src/a.tsx");
        fs::write(&source, "import \"./app.css\";\nexport const a = 1;\n").expect("source");
        let discovery = discover(root, &source, None, None, None, None).expect("discovery");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("packages/app/src/app.css").as_path())
        );
        assert!(
            discovery
                .dependencies
                .iter()
                .any(|path| path == &root.join("theme.css")),
            "{discovery:?}"
        );
    }

    #[test]
    fn explicit_paths_resolve_against_the_declaring_config_layer() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path();
        write_workspace_root(root);
        write_package(root, "tailwindcss", "4.0.0");
        write_package_manifest(root, "packages/ui");
        fs::create_dir_all(root.join("packages/ui/styles")).expect("styles");
        fs::write(
            root.join("packages/ui/styles/theme.css"),
            "@import \"tailwindcss\";\n",
        )
        .expect("theme");
        let source = root.join("packages/ui/src/a.tsx");
        fs::create_dir_all(source.parent().unwrap()).expect("src");
        fs::write(&source, "export const a = 1;\n").expect("source");
        let discovery = discover(
            root,
            &source,
            Some(Path::new("styles/theme.css")),
            Some(&root.join("packages/ui")),
            None,
            None,
        )
        .expect("configured stylesheet");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("packages/ui/styles/theme.css").as_path())
        );

        fs::write(root.join("theme.css"), "@import \"tailwindcss\";\n").expect("workspace theme");
        let discovery = discover(
            root,
            &source,
            Some(Path::new("theme.css")),
            None,
            None,
            None,
        )
        .expect("global stylesheet");
        assert_eq!(
            discovery.stylesheet.as_deref(),
            Some(root.join("theme.css").as_path()),
            "global settings resolve against the workspace root"
        );
    }

    #[test]
    fn missing_explicit_paths_are_typed_errors() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "4.0.0");
        fs::create_dir_all(root.join("src")).expect("src");
        let source = root.join("src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        let error = discover(
            root,
            &source,
            Some(Path::new("missing.css")),
            Some(root),
            None,
            None,
        )
        .expect_err("missing stylesheet");
        assert!(
            matches!(error, BridgeError::MissingStylesheet(ref path) if path.ends_with("missing.css")),
            "{error:?}"
        );

        write_package(root, "tailwindcss", "3.4.0");
        let error = discover(
            root,
            &source,
            None,
            None,
            Some(Path::new("missing.config.js")),
            Some(root),
        )
        .expect_err("missing config");
        assert!(
            matches!(error, BridgeError::MissingConfig(ref path) if path.ends_with("missing.config.js")),
            "{error:?}"
        );
    }

    #[test]
    fn adding_a_stylesheet_invalidates_discovery() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "4.0.0");
        fs::create_dir_all(root.join("src")).expect("src");
        let source = root.join("src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        assert!(
            super::discover_project(root, Some(&source), None, None, None, None)
                .expect("initial discovery")
                .stylesheet
                .is_none()
        );
        fs::write(root.join("src/app.css"), "@import \"tailwindcss\";\n").expect("stylesheet");
        clear_discovery(root, &source, None, None, None, None);
        assert_eq!(
            super::discover_project(root, Some(&source), None, None, None, None)
                .expect("refreshed discovery")
                .stylesheet
                .as_deref(),
            Some(root.join("src/app.css").as_path())
        );
    }

    #[test]
    fn workspace_shared_theme_edit_invalidates_the_order_theme() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path();
        write_workspace_root(root);
        write_package(root, "tailwindcss", "4.0.0");
        write_package_manifest(root, "packages/app");
        fs::create_dir_all(root.join("packages/app/src")).expect("src");
        fs::write(
            root.join("packages/app/src/app.css"),
            "@import \"../../../theme.css\";\n",
        )
        .expect("app css");
        let source = root.join("packages/app/src/a.tsx");
        fs::write(&source, "import \"./app.css\";\nexport const a = 1;\n").expect("source");
        let theme = root.join("theme.css");
        fs::write(&theme, "@theme { --order: p2-first; }\n").expect("theme");
        let cache = tempfile::tempdir().expect("cache");
        let queries = Mutex::new(Vec::<Vec<String>>::new());
        let query = |_: &Project, requested: Vec<&str>| {
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(requested.iter().map(|class| (*class).to_owned()).collect());
            let theme = fs::read_to_string(&theme).expect("theme");
            let order: &[&str] = if theme.contains("p2-first") {
                &["p-2", "p-4"]
            } else {
                &["p-4", "p-2"]
            };
            Ok(fake_response_with_order(&requested, order))
        };
        let classes = vec!["p-4".to_owned(), "p-2".to_owned()];
        let first = project_order_with_cache_for_using(
            root,
            Some(&source),
            None,
            None,
            None,
            None,
            &classes,
            cache.path(),
            query,
        )
        .expect("first order");
        assert!(
            first.table.order("p-2").flatten() < first.table.order("p-4").flatten(),
            "{first:?}"
        );

        fs::write(&theme, "@theme { --order: p4-first; }\n").expect("theme edit");
        let second = project_order_with_cache_for_using(
            root,
            Some(&source),
            None,
            None,
            None,
            None,
            &classes,
            cache.path(),
            query,
        )
        .expect("second order");
        assert!(
            second.table.order("p-4").flatten() < second.table.order("p-2").flatten(),
            "{second:?}"
        );
        assert_eq!(
            queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            2
        );
    }

    #[test]
    fn discovery_cache_expires_without_a_watcher_event() {
        let project = empty_project();
        let root = project.path();
        write_package(root, "tailwindcss", "4.0.0");
        fs::create_dir_all(root.join("src")).expect("src");
        let source = root.join("src/a.tsx");
        fs::write(&source, "export const a = 1;\n").expect("source");
        assert!(
            super::discover_project(root, Some(&source), None, None, None, None)
                .expect("initial discovery")
                .stylesheet
                .is_none()
        );
        fs::write(root.join("src/app.css"), "@import \"tailwindcss\";\n").expect("stylesheet");
        std::thread::sleep(DISCOVERY_TTL + Duration::from_millis(100));
        assert_eq!(
            super::discover_project(root, Some(&source), None, None, None, None)
                .expect("expired discovery")
                .stylesheet
                .as_deref(),
            Some(root.join("src/app.css").as_path())
        );
    }
}
