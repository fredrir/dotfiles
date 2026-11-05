//! Project-wide export usage analysis and TypeScript diagnostics.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::edit::{Analysis, Applicability, Edit, Finding, Fix, Severity, Tag, apply};
use ignore::{DirEntry, WalkBuilder};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, AssignmentExpression, BindingPattern, CallExpression,
    Declaration, ExportDeclaration, ExportDefaultDeclaration, ExportDefaultDeclarationKind,
    ExportNamedDeclaration, Expression, ImportDeclarationSpecifier, ImportExpression, Statement,
    TSExportAssignment, TSImportEqualsDeclaration, TSModuleReference,
};
use oxc_ast_visit::{
    Visit,
    walk::{
        walk_assignment_expression, walk_call_expression, walk_import_expression,
        walk_ts_export_assignment, walk_ts_import_equals_declaration,
    },
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

use crate::TypeDiagnostic;
use crate::dialect::Dialect;
use crate::resolve::{
    Resolver, SOURCE_EXTENSIONS, collect_export_strings, expand_glob, glob_pattern_supported,
    resolve_target, wildcard_match,
};

const AVOIDED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".git",
    ".hg",
    ".svn",
    "coverage",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".angular",
    ".turbo",
    ".yarn",
    ".pnpm",
    ".vercel",
    ".cache",
    ".venv",
    "target",
];
const SOURCE_LIST_TTL: Duration = Duration::from_millis(250);
const MAX_SOURCE_SIZE: u64 = 4 * 1024 * 1024;
const TSC_TIMEOUT: Duration = Duration::from_secs(30);
const TSC_TIMEOUT_POLL: Duration = Duration::from_millis(10);
/// Synthetic diagnostic code for a compiler invocation that exceeded the
/// timeout. Real TypeScript diagnostics start at TS1000.
pub const TSC_TIMEOUT_CODE: u32 = 0;
const CONSUMER_EXTENSIONS: &[&str] = &["vue", "svelte", "astro", "mdx"];
const NON_CODE_EXTENSIONS: &[&str] = &[
    "css", "scss", "sass", "less", "styl", "json", "html", "svg", "png", "jpg", "jpeg", "gif",
    "webp", "avif", "ico", "woff", "woff2", "ttf", "eot", "pdf", "txt", "yaml", "yml", "toml",
];
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];
const UNUSED_DIAGNOSTIC_CODES: &[u32] = &[6133, 6138, 6192, 6196, 6198, 6199, 6205, 7027, 7028];

/// Finds the project root by walking up to the nearest tsconfig/jsconfig or
/// package.json.
///
/// A package that belongs to a workspace resolves to the nearest workspace
/// root instead, so the graph includes consumers in sibling packages. A
/// workspace root is a directory with `package.json#workspaces`,
/// `pnpm-workspace.yaml`, `turbo.json`, `lerna.json`, or a solution-style
/// tsconfig (`files: []` plus `references`).
pub fn discover_root(start: &Path) -> Option<PathBuf> {
    let mut directory = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    let mut config = None;
    let mut package = None;
    let mut workspace = None;
    loop {
        if config.is_none()
            && (directory.join("tsconfig.json").is_file()
                || directory.join("jsconfig.json").is_file())
        {
            config = Some(directory.clone());
        }
        if package.is_none() && directory.join("package.json").is_file() {
            package = Some(directory.clone());
        }
        if workspace.is_none() && is_workspace_root(&directory) {
            workspace = Some(directory.clone());
        }
        let Some(parent) = directory.parent() else {
            return workspace.or(config).or(package);
        };
        directory = parent.to_path_buf();
    }
}

/// Whether `directory` declares a package workspace or a solution-style
/// TypeScript project.
fn is_workspace_root(directory: &Path) -> bool {
    ["pnpm-workspace.yaml", "turbo.json", "lerna.json"]
        .iter()
        .any(|name| directory.join(name).is_file())
        || package_declares_workspaces(directory)
        || crate::tsconfig::is_solution_config_file(&directory.join("tsconfig.json"))
}

fn package_declares_workspaces(directory: &Path) -> bool {
    let Ok(text) = fs::read_to_string(directory.join("package.json")) else {
        return false;
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    package.get("workspaces").is_some_and(|workspaces| {
        workspaces
            .as_array()
            .is_some_and(|members| !members.is_empty())
            || workspaces.as_object().is_some_and(|config| {
                config
                    .get("packages")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|members| !members.is_empty())
            })
    })
}

/// The nearest tsconfig/jsconfig for a source file, if any.
pub fn tsconfig_for(path: &Path) -> Option<PathBuf> {
    crate::tsconfig::nearest(path)
}

/// Reports exports in `path` that are never imported by another file in the
/// project. Entry points and declaration files are exempt.
pub fn unused_exports(path: &Path) -> Result<Analysis, Diagnostic> {
    let mut analysis = unused_exports_stripped(path)?;
    if fs::read(path).is_ok_and(|bytes| bytes.starts_with(b"\xEF\xBB\xBF")) {
        offset_analysis(&mut analysis, '\u{feff}'.len_utf8());
    }
    Ok(analysis)
}

pub(crate) fn unused_exports_for_analysis(path: &Path) -> Result<Analysis, Diagnostic> {
    unused_exports_stripped(path)
}

/// Whether `text` is exactly the file's on-disk contents (BOM stripped).
///
/// Project analysis reads the file for export spans and for the post-edit
/// re-parse; a dirty buffer or an in-flight format pass has different
/// coordinates, so project findings and fixes are skipped until the file is
/// saved.
pub(crate) fn buffer_matches_disk(path: &Path, text: &str) -> bool {
    fs::read_to_string(path)
        .is_ok_and(|disk| disk.strip_prefix('\u{feff}').unwrap_or(&disk) == text)
}

fn offset_analysis(analysis: &mut Analysis, offset: usize) {
    for finding in &mut analysis.findings {
        finding.start += offset;
        finding.end += offset;
        for related in &mut finding.related {
            related.start += offset;
            related.end += offset;
        }
    }
    for fix in &mut analysis.fixes {
        for edit in &mut fix.edits {
            edit.start += offset;
            edit.end += offset;
        }
        if let Some(finding) = &mut fix.finding {
            finding.start += offset;
            finding.end += offset;
            for related in &mut finding.related {
                related.start += offset;
                related.end += offset;
            }
        }
    }
}

fn unused_exports_stripped(path: &Path) -> Result<Analysis, Diagnostic> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let Some(root) = discover_root(path).or_else(|| canonical.parent().map(Path::to_path_buf))
    else {
        return Ok(Analysis::default());
    };
    let graph = graph_for(&root)?;
    Ok(graph.analysis_for(&canonical))
}

/// Clears every cached project graph, source listing and TypeScript result.
pub fn clear_cache() {
    if let Some(cache) = GRAPH_CACHE.get() {
        lock(cache).clear();
    }
    if let Some(cache) = SOURCE_LISTINGS.get() {
        lock(cache).clear();
    }
    if let Some(cache) = MODULE_CACHE.get() {
        lock(cache).clear();
    }
    if let Some(cache) = TSC_CACHE.get() {
        lock(&cache.diagnostics).clear();
    }
}

/// Drops the cached file list, graph and module entries for `root`.
///
/// Call this after creating, renaming or deleting files so lookups do not wait
/// for the short source-listing TTL.
pub fn invalidate_root(root: &Path) {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if let Some(cache) = SOURCE_LISTINGS.get() {
        lock(cache).remove(&root);
    }
    if let Some(cache) = GRAPH_CACHE.get() {
        lock(cache).remove(&root);
    }
    if let Some(cache) = MODULE_CACHE.get() {
        lock(cache).retain(|path, _| !path.starts_with(&root));
    }
}

/// The severity dotfmt assigns to a TypeScript compiler diagnostic.
///
/// Unused-code diagnostics are reported as warnings so callers can treat them
/// as formatter findings rather than build errors.
pub fn type_diagnostic_severity(code: u32) -> Severity {
    if UNUSED_DIAGNOSTIC_CODES.contains(&code) {
        Severity::Warning
    } else {
        Severity::Error
    }
}

/// Whether `code` is one of the TypeScript unused-code diagnostics.
pub fn is_unused_diagnostic(code: u32) -> bool {
    UNUSED_DIAGNOSTIC_CODES.contains(&code)
}

/// One resolved TypeScript compiler invocation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TscProgram {
    pub program: PathBuf,
    /// Arguments placed before the standard `--noEmit ...` flags (for example
    /// the path of `tsc.js` when the compiler is run through `node`).
    pub arguments: Vec<String>,
}

/// Resolves the compiler used for type-aware diagnostics.
///
/// Order: `DOTFMT_TSC`, the nearest `node_modules/typescript/lib/tsc.js`
/// (through `node`), the nearest `node_modules/.bin/tsc`, the nearest
/// `@typescript/native-preview` `tsgo`, `tsgo` on `PATH`, then `tsc` on `PATH`.
pub fn resolve_compiler(project_root: &Path) -> TscProgram {
    if let Some(value) = std::env::var_os("DOTFMT_TSC") {
        return compiler_program(PathBuf::from(value));
    }
    let root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    if let Some(path) = find_up(&root, "node_modules/typescript/lib/tsc.js") {
        return compiler_program(path);
    }
    if let Some(path) = find_up(&root, "node_modules/.bin/tsc") {
        return compiler_program(path);
    }
    for relative in [
        "node_modules/@typescript/native-preview/bin/tsgo",
        "node_modules/@typescript/native-preview/bin/tsgo.js",
    ] {
        if let Some(path) = find_up(&root, relative) {
            return compiler_program(path);
        }
    }
    if let Some(path) = find_on_path("tsgo") {
        return compiler_program(path);
    }
    TscProgram {
        program: PathBuf::from("tsc"),
        arguments: Vec::new(),
    }
}

fn compiler_program(path: PathBuf) -> TscProgram {
    let is_script = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "cjs" | "mjs"
            )
        });
    if is_script {
        TscProgram {
            program: std::env::var_os("DOTFMT_NODE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("node")),
            arguments: vec![path.to_string_lossy().into_owned()],
        }
    } else {
        TscProgram {
            program: path,
            arguments: Vec::new(),
        }
    }
}

fn find_up(start: &Path, relative: &str) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|directory| directory.join(relative))
        .find(|candidate| candidate.is_file())
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(executable_name(name));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn executable_name(name: &str) -> String {
    #[cfg(windows)]
    {
        for extension in ["cmd", "exe", "bat"] {
            let candidate = format!("{name}.{extension}");
            if std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|directory| directory.join(&candidate).is_file())
            }) {
                return candidate;
            }
        }
    }
    name.to_owned()
}

/// Runs `tsc --noEmit` for `tsconfig` and parses TypeScript diagnostics.
///
/// Results are cached by project and invalidated by any source or tsconfig
/// change. One compiler run is shared by every file in the project.
pub fn type_diagnostics(
    project_root: &Path,
    tsconfig: &Path,
) -> Result<Vec<TypeDiagnostic>, Diagnostic> {
    let diagnostics = cached_type_diagnostics(project_root, tsconfig, None, TSC_TIMEOUT)?;
    Ok(diagnostics
        .iter()
        .map(|(_, diagnostic)| diagnostic.clone())
        .collect())
}

/// Runs `tsc --noEmit` with an explicit timeout. A compiler that does not exit
/// within `timeout` is killed and reported as a timeout diagnostic instead of
/// blocking the caller forever.
pub fn type_diagnostics_with_timeout(
    project_root: &Path,
    tsconfig: &Path,
    timeout: Duration,
) -> Result<Vec<TypeDiagnostic>, Diagnostic> {
    let diagnostics = cached_type_diagnostics(project_root, tsconfig, None, timeout)?;
    Ok(diagnostics
        .iter()
        .map(|(_, diagnostic)| diagnostic.clone())
        .collect())
}

/// Returns the project diagnostics that belong to `path`.
///
/// The compiler runs once per project; subsequent calls reuse the result until
/// a source or tsconfig file changes. A project-wide timeout is attributed to
/// the requested file so it is not silently dropped by the path filter.
pub fn type_diagnostics_for(
    path: &Path,
    project_root: &Path,
    tsconfig: &Path,
) -> Result<Vec<TypeDiagnostic>, Diagnostic> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let diagnostics = cached_type_diagnostics(project_root, tsconfig, None, TSC_TIMEOUT)?;
    let mut selected: Vec<TypeDiagnostic> = diagnostics
        .iter()
        .filter_map(|(file, diagnostic)| (file == &canonical).then_some(diagnostic.clone()))
        .collect();
    if selected.is_empty()
        && let Some(timeout) = diagnostics
            .iter()
            .find(|(_, diagnostic)| diagnostic.code == TSC_TIMEOUT_CODE)
            .map(|(_, diagnostic)| diagnostic.clone())
    {
        selected.push(TypeDiagnostic {
            start: 0,
            end: 0,
            ..timeout
        });
    }
    Ok(selected)
}

/// Whether the project graph for `path` resolved every source file and import
/// specifier it found. Project-wide fixes must not be applied to an incomplete
/// graph because a missing module could still consume the exports.
pub fn graph_complete(path: &Path) -> bool {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let Some(root) = discover_root(path).or_else(|| canonical.parent().map(Path::to_path_buf))
    else {
        return false;
    };
    graph_for(&root).is_ok_and(|graph| graph.complete)
}

#[derive(Clone, Debug)]
struct Graph {
    root: PathBuf,
    modules: HashMap<PathBuf, Arc<Module>>,
    entries: HashSet<PathBuf>,
    uses: HashMap<PathBuf, ModuleUses>,
    complete: bool,
}

#[derive(Clone, Debug, Default)]
struct Module {
    exports: Vec<Export>,
    comments: Vec<(usize, usize)>,
    edges: Vec<Edge>,
    /// A dynamic import/require/glob whose target is not statically known.
    unknown_dynamic: bool,
}

/// One exported binding plus the spans a fix needs.
///
/// `start`/`end` are the identifier span used by findings. `prefix` is the
/// `export `/`export default ` range a keyword fix deletes, and `specifier` is
/// the `export { ... }` specifier a named fix removes.
#[derive(Clone, Debug)]
struct Export {
    name: String,
    start: usize,
    end: usize,
    statement_start: usize,
    statement_end: usize,
    prefix: Option<(usize, usize)>,
    specifier: Option<(usize, usize)>,
    kind: ExportKind,
    decorated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportKind {
    Keyword,
    Specifier,
    DefaultNamed,
    DefaultExpression,
}

#[derive(Clone, Debug)]
struct Edge {
    specifier: String,
    kind: EdgeKind,
}

#[derive(Clone, Debug)]
enum EdgeKind {
    Named(Vec<String>),
    Default,
    Namespace,
    All,
    SideEffect,
    Glob(Vec<String>),
}

impl Graph {
    fn analysis_for(&self, path: &Path) -> Analysis {
        let Some(module) = self.modules.get(path) else {
            return Analysis::default();
        };
        if self.entries.contains(path) || is_declaration_file(path) {
            return Analysis::default();
        }
        // Spans and safety checks are computed against the file the graph was
        // built from. Callers must not request fixes for a dirty buffer.
        let Ok(text) = fs::read_to_string(path) else {
            return Analysis::default();
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let use_site = self.uses.get(path);
        let all_used = use_site.is_some_and(|use_site| use_site.all);
        let mut analysis = Analysis::default();
        let mut findings: Vec<Option<Finding>> = Vec::with_capacity(module.exports.len());
        for export in &module.exports {
            if all_used || use_site.is_some_and(|use_site| use_site.names.contains(&export.name)) {
                findings.push(None);
                continue;
            }
            let finding = Finding::new(
                "unused-export",
                format!("Export `{}` is never imported in this project", export.name),
                Severity::Warning,
                export.start,
                export.end,
            )
            .with_tag(Tag::Unnecessary);
            findings.push(Some(finding.clone()));
            analysis.findings.push(finding);
        }
        analysis
            .findings
            .sort_by_key(|finding| (finding.start, finding.end));
        if !self.complete || !is_private_package(path, &self.root) {
            return analysis;
        }
        export_fixes(path, text, module, &findings, &mut analysis.fixes);
        analysis
    }

    fn resolve_edges(&mut self, root: &Path) {
        let mut resolver = Resolver::new(root);
        let paths: Vec<PathBuf> = self.modules.keys().cloned().collect();
        let mut uses: HashMap<PathBuf, ModuleUses> = HashMap::new();
        for importer in paths {
            let Some(module) = self.modules.get(&importer) else {
                continue;
            };
            // A malformed nearest/solution config or a broken `extends` chain
            // hides aliases, so unresolved imports cannot be classified as
            // external.
            if resolver
                .configs_for(&importer)
                .iter()
                .any(|config| config.invalid)
            {
                self.complete = false;
            }
            for edge in &module.edges {
                if let EdgeKind::Glob(patterns) = &edge.kind {
                    for pattern in patterns {
                        // Negated patterns and glob syntax this resolver does
                        // not model (extglobs, character classes, broken
                        // braces) may hide a consumer, so project fixes stay
                        // disabled.
                        if pattern.trim_start().starts_with('!') || !glob_pattern_supported(pattern)
                        {
                            self.complete = false;
                        }
                        for target in expand_glob(root, &importer, pattern) {
                            let Ok(target) = fs::canonicalize(&target) else {
                                continue;
                            };
                            if self.modules.contains_key(&target) {
                                uses.entry(target).or_default().all = true;
                            }
                        }
                    }
                    continue;
                }
                let Some(target) = resolver.resolve(&importer, &edge.specifier) else {
                    if !ignored_specifier(&edge.specifier)
                        && local_specifier(&mut resolver, root, &importer, &edge.specifier)
                    {
                        self.complete = false;
                    }
                    continue;
                };
                let target = fs::canonicalize(&target).unwrap_or(target);
                if inside_node_modules(&target) {
                    // Installed packages are outside the project graph; their
                    // imports cannot consume this project's exports.
                    continue;
                }
                if !self.modules.contains_key(&target) {
                    // A local module the graph could not include (declaration
                    // file, generated file, or a path outside the walked root)
                    // means an unseen consumer may exist.
                    if !target.starts_with(root)
                        || is_declaration_file(&target)
                        || local_specifier(&mut resolver, root, &importer, &edge.specifier)
                    {
                        self.complete = false;
                    }
                    continue;
                }
                let use_site = uses.entry(target).or_default();
                match &edge.kind {
                    EdgeKind::Named(names) => use_site.names.extend(names.iter().cloned()),
                    EdgeKind::Default => {
                        use_site.names.insert("default".to_owned());
                    }
                    EdgeKind::Namespace | EdgeKind::All => use_site.all = true,
                    EdgeKind::SideEffect | EdgeKind::Glob(_) => {}
                }
            }
        }
        self.uses = uses;
    }
}

/// Whether the walked root is nested in a larger project and so cannot see
/// every consumer of its exports.
fn graph_truncated(root: &Path) -> bool {
    // A workspace root already contains its member packages. An unrelated
    // ancestor package (for example a tooling manifest in a home directory)
    // does not make the workspace partial; only a nested project, or a larger
    // workspace that can consume it, does.
    let workspace = is_workspace_root(root);
    for ancestor in root.ancestors().skip(1) {
        if ancestor.join("tsconfig.json").is_file()
            || ancestor.join("jsconfig.json").is_file()
            || is_workspace_root(ancestor)
            || (!workspace && ancestor.join("package.json").is_file())
        {
            return true;
        }
    }
    let probe = root.join("__dotfmt_probe.ts");
    let config = crate::tsconfig::load_for_within(&probe, root);
    if config.invalid {
        return true;
    }
    config
        .references
        .iter()
        .any(|reference| !reference.starts_with(root))
}

fn inside_node_modules(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "node_modules")
}

/// Whether an unresolved specifier is expected to name a local module.
///
/// TypeScript treats relative paths, `#imports`, `paths` aliases and `baseUrl`
/// lookups as local. A bare package that resolves to `node_modules` is external
/// even when it is not part of the graph, so it cannot hide a consumer.
fn local_specifier(resolver: &mut Resolver, root: &Path, importer: &Path, specifier: &str) -> bool {
    let specifier = specifier.trim();
    if specifier.is_empty() {
        return false;
    }
    if specifier.starts_with('.') || specifier.starts_with('/') || specifier.starts_with('#') {
        return true;
    }
    let configs = resolver.configs_for(importer);
    if configs.iter().any(|config| config.invalid) {
        return true;
    }
    if configs.iter().any(|config| {
        config
            .paths
            .keys()
            .any(|pattern| alias_matches(pattern, specifier))
    }) {
        return true;
    }
    if let Some(package) = node_modules_package(importer, specifier) {
        let package = fs::canonicalize(&package).unwrap_or(package);
        return package.starts_with(root) && !inside_node_modules(&package);
    }
    // `baseUrl` makes bare specifiers project-local unless an installed package
    // already owns the name.
    configs.iter().any(|config| config.base_url.is_some())
}

fn alias_matches(pattern: &str, specifier: &str) -> bool {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            specifier.len() >= prefix.len() + suffix.len()
                && specifier.starts_with(prefix)
                && specifier.ends_with(suffix)
        }
        None => pattern == specifier,
    }
}

fn node_modules_package(importer: &Path, specifier: &str) -> Option<PathBuf> {
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    let name = if first.starts_with('@') {
        format!("{first}/{}", parts.next()?)
    } else {
        first.to_owned()
    };
    let mut directory = importer.parent()?;
    loop {
        let package = directory.join("node_modules").join(&name);
        if package.is_dir() {
            return Some(package);
        }
        directory = directory.parent()?;
    }
}

/// Builds one fix per unused export where a deletion is provably safe.
///
/// Statement-wide strategies (an all-unused specifier list or a multi-declarator
/// declaration) attach their single edit to the first unused export so applying
/// every fix cannot overlap.
fn export_fixes(
    path: &Path,
    text: &str,
    module: &Module,
    findings: &[Option<Finding>],
    fixes: &mut Vec<Fix>,
) {
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, export) in module.exports.iter().enumerate() {
        groups
            .entry(export.statement_start)
            .or_default()
            .push(index);
    }
    for indexes in groups.values() {
        let first = &module.exports[indexes[0]];
        let unused: Vec<usize> = indexes
            .iter()
            .copied()
            .filter(|index| findings[*index].is_some())
            .collect();
        if unused.is_empty()
            || first.decorated
            || has_ignore_marker(
                text,
                &module.comments,
                first.statement_start,
                first.statement_end,
            )
        {
            continue;
        }
        match first.kind {
            // Deleting the whole statement would remove the value expression or
            // an anonymous default declaration, so keep the finding only.
            ExportKind::DefaultExpression => {}
            ExportKind::DefaultNamed | ExportKind::Keyword => {
                if first.kind == ExportKind::Keyword && unused.len() != indexes.len() {
                    // A multi-declarator statement may only lose `export ` when
                    // every name it binds is unused.
                    continue;
                }
                let Some((start, end)) = first.prefix.and_then(|range| export_prefix(text, range))
                else {
                    continue;
                };
                let index = unused[0];
                push_export_fix(
                    path,
                    text,
                    module,
                    fixes,
                    findings[index].as_ref().expect("unused finding"),
                    &first.name,
                    Edit::delete(start, end),
                );
            }
            ExportKind::Specifier => {
                if unused.len() == indexes.len() {
                    let (start, end) =
                        statement_edit(text, first.statement_start, first.statement_end);
                    let index = unused[0];
                    push_export_fix(
                        path,
                        text,
                        module,
                        fixes,
                        findings[index].as_ref().expect("unused finding"),
                        &module.exports[index].name,
                        Edit::delete(start, end),
                    );
                    continue;
                }
                let mut ordered = indexes.clone();
                ordered.sort_by_key(|index| {
                    module.exports[*index]
                        .specifier
                        .map_or(0, |(start, _)| start)
                });
                for (position, index) in ordered.iter().copied().enumerate() {
                    let Some(finding) = findings[index].as_ref() else {
                        continue;
                    };
                    let previous_kept = position > 0 && findings[ordered[position - 1]].is_none();
                    let Some((start, end)) =
                        specifier_removal(text, &module.exports, &ordered, position, previous_kept)
                    else {
                        continue;
                    };
                    push_export_fix(
                        path,
                        text,
                        module,
                        fixes,
                        finding,
                        &module.exports[index].name,
                        Edit::delete(start, end),
                    );
                }
            }
        }
    }
}

fn export_prefix(text: &str, (start, end): (usize, usize)) -> Option<(usize, usize)> {
    let prefix = text.get(start..end)?.trim();
    matches!(prefix, "export" | "export default").then_some((start, end))
}

/// The byte range that removes one specifier from `export { ... }`.
///
/// Non-final specifiers take their following comma; a final specifier takes the
/// preceding comma when the previous specifier is kept. That keeps the edits for
/// several unused specifiers non-overlapping.
fn specifier_removal(
    text: &str,
    exports: &[Export],
    ordered: &[usize],
    position: usize,
    previous_kept: bool,
) -> Option<(usize, usize)> {
    let index = *ordered.get(position)?;
    let (start, end) = exports[index].specifier?;
    text.get(start..end)?;
    let next_start = ordered
        .get(position + 1)
        .and_then(|index| exports[*index].specifier)
        .map(|(start, _)| start);
    let limit = next_start.unwrap_or(exports[index].statement_end);
    let comma = text.get(end..limit)?.find(',').map(|offset| end + offset);
    if position + 1 < ordered.len() {
        let comma = comma?;
        return Some((start, comma + 1));
    }
    let previous_comma = || {
        let previous_end = exports[*ordered.get(position - 1)?].specifier?.1;
        let offset = text.get(previous_end..start)?.rfind(',')?;
        Some(previous_end + offset)
    };
    match (comma, previous_kept && position > 0) {
        (Some(comma), true) => Some((previous_comma()?, comma + 1)),
        (Some(comma), false) => Some((start, comma + 1)),
        (None, true) => Some((previous_comma()?, end)),
        (None, false) => Some((start, end)),
    }
}

fn push_export_fix(
    path: &Path,
    text: &str,
    module: &Module,
    fixes: &mut Vec<Fix>,
    finding: &Finding,
    name: &str,
    edit: Edit,
) {
    if edit.start > edit.end
        || edit.end > text.len()
        || module
            .comments
            .iter()
            .any(|&(start, end)| start < edit.end && end > edit.start)
        || text
            .get(edit.start..edit.end)
            .is_some_and(|removed| removed.contains('@'))
    {
        return;
    }
    let Ok(candidate) = apply(text, std::slice::from_ref(&edit)) else {
        return;
    };
    if !reparses(path, &candidate) {
        return;
    }
    fixes.push(
        Fix::new(
            format!("Remove unused export `{name}`"),
            "quickfix",
            Applicability::Unsafe,
            vec![edit],
        )
        .with_finding(finding.clone()),
    );
}

pub(crate) fn reparses(path: &Path, text: &str) -> bool {
    let source_type = Dialect::Auto.source_type_for_path(path);
    let allocator = Allocator::default();
    Parser::new(&allocator, text, source_type)
        .parse()
        .diagnostics
        .is_empty()
}

fn has_ignore_marker(text: &str, comments: &[(usize, usize)], start: usize, end: usize) -> bool {
    comments.iter().any(|&(comment_start, comment_end)| {
        let marker = text.get(comment_start..comment_end).is_some_and(|comment| {
            comment.contains("prettier-ignore") || comment.contains("oxfmt-ignore")
        });
        marker
            && (leading_comment(text, comment_end, start)
                || trailing_comment(text, end, comment_start))
    })
}

fn leading_comment(text: &str, comment_end: usize, start: usize) -> bool {
    comment_end <= start
        && text
            .get(comment_end..start)
            .is_some_and(|gap| gap.trim().is_empty() && gap.matches('\n').count() <= 1)
}

fn trailing_comment(text: &str, end: usize, comment_start: usize) -> bool {
    comment_start >= end
        && text
            .get(end..comment_start)
            .is_some_and(|gap| !gap.contains('\n'))
}

/// The byte range that removes a whole statement without touching other
/// statements that share its line.
fn statement_edit(text: &str, start: usize, end: usize) -> (usize, usize) {
    let line_start = text[..start.min(text.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let start = if text[line_start..start].trim().is_empty() {
        line_start
    } else {
        start
    };
    let mut end = end;
    while end < text.len() && matches!(text.as_bytes()[end], b' ' | b'\t' | b';') {
        end += 1;
    }
    let line_end = text[end.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |index| end + index);
    if !text[end..line_end].trim().is_empty() {
        return (start, end);
    }
    if text.as_bytes().get(end) == Some(&b'\r') {
        end += 1;
    }
    if text.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    (start, end)
}

/// The nearest package.json decides whether a fix may remove public API.
///
/// A file with no package boundary inside the project root is not published as
/// a package, so it behaves like a private package.
fn is_private_package(path: &Path, root: &Path) -> bool {
    let mut directory = path.parent();
    while let Some(current) = directory {
        let package = current.join("package.json");
        if package.is_file() {
            let Ok(text) = fs::read_to_string(&package) else {
                return false;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                return false;
            };
            return value.get("private").and_then(serde_json::Value::as_bool) == Some(true);
        }
        if current == root {
            break;
        }
        directory = current.parent();
    }
    true
}

#[derive(Clone, Debug, Default)]
struct ModuleUses {
    names: BTreeSet<String>,
    all: bool,
}

struct CachedGraph {
    fingerprint: u64,
    graph: Arc<Graph>,
}

struct SourceListing {
    files: Vec<PathBuf>,
    metadata: Vec<PathBuf>,
}

struct CachedListing {
    cached_at: Instant,
    listing: Arc<SourceListing>,
}

struct CachedModule {
    len: u64,
    modified: Option<SystemTime>,
    module: Arc<Module>,
    complete: bool,
}

type GraphCache = Mutex<HashMap<PathBuf, CachedGraph>>;
type ListingCache = Mutex<HashMap<PathBuf, CachedListing>>;
type ModuleCache = Mutex<HashMap<PathBuf, CachedModule>>;

static GRAPH_CACHE: OnceLock<GraphCache> = OnceLock::new();
static SOURCE_LISTINGS: OnceLock<ListingCache> = OnceLock::new();
static MODULE_CACHE: OnceLock<ModuleCache> = OnceLock::new();

fn graph_for(root: &Path) -> Result<Arc<Graph>, Diagnostic> {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let listing = source_listing(&root);
    let fingerprint = fingerprint(&listing);
    let cache = GRAPH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = lock(cache).get(&root)
        && cached.fingerprint == fingerprint
    {
        return Ok(Arc::clone(&cached.graph));
    }
    let graph = Arc::new(build_graph(&root, &listing));
    lock(cache).insert(
        root,
        CachedGraph {
            fingerprint,
            graph: Arc::clone(&graph),
        },
    );
    Ok(graph)
}

fn source_listing(root: &Path) -> Arc<SourceListing> {
    let cache = SOURCE_LISTINGS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = lock(cache).get(root)
        && cached.cached_at.elapsed() < SOURCE_LIST_TTL
    {
        return Arc::clone(&cached.listing);
    }
    let listing = Arc::new(walk_source_listing(root));
    lock(cache).insert(
        root.to_path_buf(),
        CachedListing {
            cached_at: Instant::now(),
            listing: Arc::clone(&listing),
        },
    );
    listing
}

fn walk_source_listing(root: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(true)
        .filter_entry(|entry| !avoided(entry));
    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if is_source_file(entry.path())
            || is_consumer_file(entry.path())
            || is_declaration_file(entry.path())
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();

    let mut directories = BTreeSet::from([root.to_path_buf()]);
    for file in &files {
        let mut directory = file.parent();
        while let Some(current) = directory {
            directories.insert(current.to_path_buf());
            if current == root {
                break;
            }
            directory = current.parent();
        }
    }
    let mut metadata = Vec::new();
    for directory in directories {
        for name in [
            "package.json",
            "tsconfig.json",
            "jsconfig.json",
            "index.html",
        ] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                if matches!(name, "tsconfig.json" | "jsconfig.json") {
                    metadata.extend(crate::tsconfig::load_at(&candidate).chain);
                }
                metadata.push(candidate);
            }
        }
    }
    metadata.sort();
    metadata.dedup();
    SourceListing { files, metadata }
}

fn fingerprint(listing: &SourceListing) -> u64 {
    let mut hasher = DefaultHasher::new();
    for path in listing.files.iter().chain(listing.metadata.iter()) {
        hash_file(&mut hasher, path);
    }
    hasher.finish()
}

fn hash_file(hasher: &mut DefaultHasher, path: &Path) {
    path.hash(hasher);
    if let Ok(metadata) = fs::metadata(path) {
        metadata.len().hash(hasher);
        metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .hash(hasher);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn build_graph(root: &Path, listing: &SourceListing) -> Graph {
    let mut modules = HashMap::new();
    // A graph rooted at a package inside a larger workspace cannot see the
    // package's consumers, so project fixes must stay off.
    let mut complete = !graph_truncated(root);
    for path in &listing.files {
        let Some(loaded) = load_module(path) else {
            complete = false;
            continue;
        };
        complete &= loaded.complete;
        if let Some(module) = loaded.module {
            modules.insert(loaded.key, module);
        }
    }
    let entries = entry_points(root, &listing.files);
    let mut graph = Graph {
        root: root.to_path_buf(),
        modules,
        entries,
        uses: HashMap::new(),
        complete,
    };
    graph.resolve_edges(root);
    graph
}

struct LoadedModule {
    key: PathBuf,
    module: Option<Arc<Module>>,
    complete: bool,
}

fn load_module(path: &Path) -> Option<LoadedModule> {
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::metadata(path).ok()?;
    let len = metadata.len();
    let modified = metadata.modified().ok();
    let cache = MODULE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = lock(cache).get(&canonical)
        && cached.len == len
        && cached.modified == modified
    {
        return Some(LoadedModule {
            key: canonical,
            module: Some(Arc::clone(&cached.module)),
            complete: cached.complete,
        });
    }
    let (module, complete) = if len > MAX_SOURCE_SIZE {
        (Module::default(), false)
    } else {
        match fs::read_to_string(path) {
            Ok(text) if is_consumer_file(path) => (scan_consumer_module(&text), true),
            Ok(text) => match parse_module(path, &text) {
                Some(module) => {
                    let complete = !module.unknown_dynamic;
                    (module, complete)
                }
                None => (Module::default(), false),
            },
            Err(_) => (Module::default(), false),
        }
    };
    let module = Arc::new(module);
    lock(cache).insert(
        canonical.clone(),
        CachedModule {
            len,
            modified,
            module: Arc::clone(&module),
            complete,
        },
    );
    Some(LoadedModule {
        key: canonical,
        module: Some(module),
        complete,
    })
}

fn avoided(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| AVOIDED_DIRECTORIES.contains(&name))
}

fn is_source_file(path: &Path) -> bool {
    if is_declaration_file(path) {
        return false;
    }
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn is_declaration_file(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            name.ends_with(".d.ts")
                || name.ends_with(".d.mts")
                || name.ends_with(".d.cts")
                || name.ends_with(".d.tsx")
        })
}

fn is_consumer_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            CONSUMER_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn ignored_specifier(specifier: &str) -> bool {
    let specifier = specifier.trim();
    if specifier.is_empty()
        || specifier.starts_with("node:")
        || specifier.starts_with("http://")
        || specifier.starts_with("https://")
        || specifier.starts_with("data:")
        || specifier.starts_with("virtual:")
        || specifier.contains('?')
    {
        return true;
    }
    let package = specifier
        .split_once('/')
        .map_or(specifier, |(name, _)| name);
    if NODE_BUILTINS.contains(&package) {
        return true;
    }
    Path::new(specifier)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            NON_CODE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// Collects import edges from component files that are not parsed as
/// TypeScript (`.vue`, `.svelte`, `.astro`, `.mdx`).
fn scan_consumer_module(text: &str) -> Module {
    let mut module = Module {
        unknown_dynamic: text.contains("import(") || text.contains("require("),
        ..Module::default()
    };
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        if !matches!(quote, b'"' | b'\'' | b'`') {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() {
            if bytes[end] == b'\\' {
                end += 2;
                continue;
            }
            if bytes[end] == quote {
                break;
            }
            if bytes[end] == b'\n' && quote != b'`' {
                break;
            }
            end += 1;
        }
        if end < bytes.len()
            && bytes[end] == quote
            && let Some(specifier) = text.get(start..end)
            && is_import_specifier(specifier)
            && is_import_context(&text[..index])
        {
            module.edges.push(Edge {
                specifier: specifier.to_owned(),
                kind: EdgeKind::Namespace,
            });
        }
        index = end.saturating_add(1);
    }
    module
}

fn is_import_specifier(specifier: &str) -> bool {
    !specifier.is_empty()
        && !specifier.contains(char::is_whitespace)
        && !specifier.contains('\n')
        && specifier.chars().next().is_some_and(|first| {
            first.is_ascii_alphanumeric() || matches!(first, '.' | '/' | '@' | '#' | '~')
        })
}

fn is_import_context(prefix: &str) -> bool {
    let before = prefix.trim_end();
    before.ends_with("from")
        || before.ends_with("import")
        || before.ends_with("require")
        || before.ends_with("require(")
        || before.ends_with("import(")
}

fn parse_module(path: &Path, text: &str) -> Option<Module> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let source_type =
        SourceType::from_path(path).unwrap_or_else(|_| Dialect::Auto.source_type_for_path(path));
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut module = Module {
        comments: parsed
            .program
            .comments
            .iter()
            .map(|comment| (comment.span.start as usize, comment.span.end as usize))
            .collect(),
        ..Module::default()
    };
    for statement in &parsed.program.body {
        match statement {
            Statement::ImportDeclaration(declaration) => {
                if let Some(edge) = import_edge(declaration) {
                    module.edges.push(edge);
                }
            }
            Statement::ExportFromDeclaration(declaration) => {
                let mut names = Vec::new();
                for specifier in &declaration.specifiers {
                    names.push(specifier.local.name().as_str().to_owned());
                }
                if names.is_empty() {
                    module.edges.push(Edge {
                        specifier: declaration
                            .source
                            .value
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        kind: EdgeKind::SideEffect,
                    });
                } else {
                    module.edges.push(Edge {
                        specifier: declaration
                            .source
                            .value
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        kind: EdgeKind::Named(names),
                    });
                }
            }
            Statement::ExportAllDeclaration(declaration) => {
                module.edges.push(Edge {
                    specifier: declaration
                        .source
                        .value
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    kind: if declaration.exported.is_some() {
                        EdgeKind::Namespace
                    } else {
                        EdgeKind::All
                    },
                });
            }
            Statement::ExportDeclaration(declaration) => {
                collect_declaration_export(declaration, &mut module.exports);
            }
            Statement::ExportNamedDeclaration(declaration) => {
                collect_named_export(declaration, &mut module.exports);
            }
            Statement::ExportDefaultDeclaration(declaration) => {
                collect_default_export(text, declaration, &mut module.exports);
            }
            _ => {}
        }
    }
    let mut dynamic = DynamicEdges {
        text,
        edges: Vec::new(),
        unknown: false,
    };
    dynamic.visit_program(&parsed.program);
    module.edges.extend(dynamic.edges);
    module.unknown_dynamic = dynamic.unknown;
    Some(module)
}

fn import_edge(declaration: &oxc_ast::ast::ImportDeclaration<'_>) -> Option<Edge> {
    let specifier = declaration
        .source
        .value
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let Some(specifiers) = declaration.specifiers.as_ref() else {
        return Some(Edge {
            specifier,
            kind: EdgeKind::SideEffect,
        });
    };
    if specifiers.is_empty() {
        return Some(Edge {
            specifier,
            kind: EdgeKind::SideEffect,
        });
    }
    let mut names = Vec::new();
    let mut has_default = false;
    let mut has_namespace = false;
    for specifier in specifiers {
        match specifier {
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                names.push(specifier.imported.name().as_str().to_owned());
            }
            ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => has_default = true,
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => has_namespace = true,
        }
    }
    if has_namespace {
        return Some(Edge {
            specifier,
            kind: EdgeKind::Namespace,
        });
    }
    if has_default && names.is_empty() {
        return Some(Edge {
            specifier,
            kind: EdgeKind::Default,
        });
    }
    if has_default {
        names.push("default".to_owned());
    }
    Some(Edge {
        specifier,
        kind: EdgeKind::Named(names),
    })
}

#[derive(Clone, Copy)]
struct ExportLayout {
    statement_start: usize,
    statement_end: usize,
    prefix: (usize, usize),
    kind: ExportKind,
    decorated: bool,
}

fn collect_declaration_export(declaration: &ExportDeclaration<'_>, exports: &mut Vec<Export>) {
    let layout = ExportLayout {
        statement_start: declaration.span.start as usize,
        statement_end: declaration.span.end as usize,
        prefix: (
            declaration.span.start as usize,
            declaration.declaration.span().start as usize,
        ),
        kind: ExportKind::Keyword,
        decorated: matches!(
            &declaration.declaration,
            Declaration::ClassDeclaration(class) if !class.decorators.is_empty()
        ),
    };
    match &declaration.declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                collect_binding_exports(&declarator.id, layout, exports);
            }
        }
        declaration => {
            if let Some(identifier) = declaration.id() {
                push_export(
                    exports,
                    identifier.name.as_str().to_owned(),
                    identifier.span.start as usize,
                    identifier.span.end as usize,
                    layout,
                );
            }
        }
    }
}

fn collect_named_export(declaration: &ExportNamedDeclaration<'_>, exports: &mut Vec<Export>) {
    let statement_start = declaration.span.start as usize;
    let statement_end = declaration.span.end as usize;
    for specifier in &declaration.specifiers {
        let exported = specifier.exported.span();
        exports.push(Export {
            name: specifier.exported.name().as_str().to_owned(),
            start: exported.start as usize,
            end: exported.end as usize,
            statement_start,
            statement_end,
            prefix: None,
            specifier: Some((specifier.span.start as usize, specifier.span.end as usize)),
            kind: ExportKind::Specifier,
            decorated: false,
        });
    }
}

fn collect_default_export(
    text: &str,
    declaration: &ExportDefaultDeclaration<'_>,
    exports: &mut Vec<Export>,
) {
    let kind = match &declaration.declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(function) if function.id.is_some() => {
            ExportKind::DefaultNamed
        }
        ExportDefaultDeclarationKind::ClassDeclaration(class) if class.id.is_some() => {
            ExportKind::DefaultNamed
        }
        ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => return,
        _ => ExportKind::DefaultExpression,
    };
    let statement_start = declaration.span.start as usize;
    let statement_end = declaration.span.end as usize;
    let offset = text
        .get(statement_start..statement_end)
        .and_then(|text| text.find("default"))
        .map_or(statement_start, |offset| statement_start + offset);
    exports.push(Export {
        name: "default".to_owned(),
        start: offset,
        end: offset + "default".len(),
        statement_start,
        statement_end,
        prefix: Some((
            statement_start,
            declaration.declaration.span().start as usize,
        )),
        specifier: None,
        kind,
        decorated: matches!(
            &declaration.declaration,
            ExportDefaultDeclarationKind::ClassDeclaration(class)
                if !class.decorators.is_empty()
        ),
    });
}

fn push_export(
    exports: &mut Vec<Export>,
    name: String,
    start: usize,
    end: usize,
    layout: ExportLayout,
) {
    exports.push(Export {
        name,
        start,
        end,
        statement_start: layout.statement_start,
        statement_end: layout.statement_end,
        prefix: Some(layout.prefix),
        specifier: None,
        kind: layout.kind,
        decorated: layout.decorated,
    });
}

fn collect_binding_exports(
    pattern: &BindingPattern<'_>,
    layout: ExportLayout,
    exports: &mut Vec<Export>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => push_export(
            exports,
            identifier.name.as_str().to_owned(),
            identifier.span.start as usize,
            identifier.span.end as usize,
            layout,
        ),
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_binding_exports(&property.value, layout, exports);
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_exports(&rest.argument, layout, exports);
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_binding_exports(element, layout, exports);
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_exports(&rest.argument, layout, exports);
            }
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_binding_exports(&pattern.left, layout, exports);
        }
    }
}

struct DynamicEdges<'t> {
    text: &'t str,
    edges: Vec<Edge>,
    unknown: bool,
}

/// A string literal or a template literal with no substitutions.
fn static_specifier(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::StringLiteral(literal) => literal.value.as_str().map(str::to_owned),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .map(|quasi| quasi.value.raw.as_str().to_owned()),
        _ => None,
    }
}

impl<'a> Visit<'a> for DynamicEdges<'_> {
    fn visit_call_expression(&mut self, expression: &CallExpression<'a>) {
        if let Expression::Identifier(callee) = &expression.callee
            && callee.name.as_str() == "require"
            && let Some(argument) = expression.arguments.first()
        {
            match string_argument(argument) {
                Some(value) => self.edges.push(Edge {
                    specifier: value,
                    kind: EdgeKind::Namespace,
                }),
                None => self.unknown = true,
            }
        }
        if is_import_meta_glob(&expression.callee) {
            let mut patterns = Vec::new();
            if let Some(argument) = expression.arguments.first() {
                match argument {
                    Argument::StringLiteral(literal) => {
                        if let Some(value) = literal.value.as_str() {
                            patterns.push(value.to_owned());
                        }
                    }
                    Argument::ArrayExpression(array) => {
                        for element in &array.elements {
                            if let ArrayExpressionElement::StringLiteral(literal) = element
                                && let Some(value) = literal.value.as_str()
                            {
                                patterns.push(value.to_owned());
                            }
                        }
                    }
                    _ => self.unknown = true,
                }
            } else {
                self.unknown = true;
            }
            if !patterns.is_empty() {
                self.edges.push(Edge {
                    specifier: String::new(),
                    kind: EdgeKind::Glob(patterns),
                });
            }
        }
        walk_call_expression(self, expression);
    }

    fn visit_import_expression(&mut self, expression: &ImportExpression<'a>) {
        match static_specifier(&expression.source) {
            Some(value) => self.edges.push(Edge {
                specifier: value,
                kind: EdgeKind::Namespace,
            }),
            None => self.unknown = true,
        }
        walk_import_expression(self, expression);
    }

    fn visit_ts_import_equals_declaration(&mut self, declaration: &TSImportEqualsDeclaration<'a>) {
        if let TSModuleReference::ExternalModuleReference(reference) = &declaration.module_reference
            && let Some(value) = reference.expression.value.as_str()
        {
            self.edges.push(Edge {
                specifier: value.to_owned(),
                kind: EdgeKind::Namespace,
            });
        }
        walk_ts_import_equals_declaration(self, declaration);
    }

    fn visit_assignment_expression(&mut self, expression: &AssignmentExpression<'a>) {
        if expression.left.span().source_text(self.text) == "module.exports"
            && let Expression::StringLiteral(literal) = &expression.right
            && let Some(value) = literal.value.as_str()
        {
            self.edges.push(Edge {
                specifier: value.to_owned(),
                kind: EdgeKind::Namespace,
            });
        }
        walk_assignment_expression(self, expression);
    }

    fn visit_ts_export_assignment(&mut self, assignment: &TSExportAssignment<'a>) {
        if let Expression::StringLiteral(literal) = &assignment.expression
            && let Some(value) = literal.value.as_str()
        {
            self.edges.push(Edge {
                specifier: value.to_owned(),
                kind: EdgeKind::Namespace,
            });
        }
        walk_ts_export_assignment(self, assignment);
    }
}

fn string_argument(argument: &Argument<'_>) -> Option<String> {
    match argument {
        Argument::StringLiteral(literal) => literal.value.as_str().map(str::to_owned),
        _ => None,
    }
}

fn is_import_meta_glob(expression: &Expression<'_>) -> bool {
    matches!(
        expression,
        Expression::StaticMemberExpression(member)
            if matches!(member.property.name.as_str(), "glob" | "globEager")
                && matches!(member.object, Expression::ImportMeta(_))
    )
}

fn entry_points(root: &Path, files: &[PathBuf]) -> HashSet<PathBuf> {
    let mut entries = HashSet::new();
    let mut directories = BTreeSet::from([root.to_path_buf()]);
    for file in files {
        let mut directory = file.parent();
        while let Some(current) = directory {
            directories.insert(current.to_path_buf());
            if current == root {
                break;
            }
            directory = current.parent();
        }
    }
    for directory in &directories {
        package_entries(directory, files, &mut entries);
        index_html_entries(directory, root, &mut entries);
    }
    for file in files {
        if is_entry_file(root, file) {
            insert_entry(&mut entries, file);
        }
    }
    for extension in SOURCE_EXTENSIONS {
        insert_entry(
            &mut entries,
            &root.join("src").join(format!("index.{extension}")),
        );
        insert_entry(&mut entries, &root.join(format!("index.{extension}")));
    }
    entries
}

fn insert_entry(entries: &mut HashSet<PathBuf>, path: &Path) {
    if let Some(canonical) = resolve_target(path) {
        entries.insert(canonical);
    }
}

fn package_entries(directory: &Path, files: &[PathBuf], entries: &mut HashSet<PathBuf>) {
    let Ok(text) = fs::read_to_string(directory.join("package.json")) else {
        return;
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let mut values = Vec::new();
    for key in ["main", "module", "types", "typings"] {
        if let Some(value) = package.get(key).and_then(serde_json::Value::as_str) {
            values.push(value.to_owned());
        }
    }
    collect_export_strings(package.get("exports"), &mut values);
    if let Some(bin) = package.get("bin") {
        collect_export_strings(Some(bin), &mut values);
    }
    for value in values {
        if value.contains('*') {
            for candidate in wildcard_package_entries(directory, &value, files) {
                insert_entry(entries, &candidate);
            }
        } else {
            for candidate in package_entry_candidates(directory, &value) {
                insert_entry(entries, &candidate);
            }
        }
    }
    if package.pointer("/engines/vscode").is_some() {
        for name in [
            "src/extension.ts",
            "src/extension.tsx",
            "extension.ts",
            "extension.tsx",
        ] {
            insert_entry(entries, &directory.join(name));
        }
    }
}

/// Files matched by a wildcard `package.json#exports` target.
///
/// Wildcard public entry points are common (`"./features/*":
/// "./src/features/*.ts"`); without expanding them a public module would be
/// reported unused and could receive an unsafe export fix.
fn wildcard_package_entries(directory: &Path, value: &str, files: &[PathBuf]) -> Vec<PathBuf> {
    let value = value.strip_prefix("./").unwrap_or(value);
    if value.is_empty() || value.starts_with("http") || !value.contains('*') {
        return Vec::new();
    }
    let mut patterns = vec![value.to_owned()];
    if let Some(source) = crate::resolve::source_directory_swap(directory, value)
        && let Ok(relative) = source.strip_prefix(directory)
    {
        patterns.push(relative.to_string_lossy().replace('\\', "/"));
    }
    let mut variants = Vec::new();
    for pattern in patterns {
        for (from, to) in [
            (".js", ".ts"),
            (".js", ".tsx"),
            (".jsx", ".tsx"),
            (".mjs", ".mts"),
            (".cjs", ".cts"),
        ] {
            if let Some(stem) = pattern.strip_suffix(from) {
                variants.push(format!("{stem}{to}"));
            }
        }
        variants.push(pattern);
    }
    let mut matched = Vec::new();
    for file in files {
        let Ok(relative) = file.strip_prefix(directory) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if variants
            .iter()
            .any(|pattern| wildcard_match(pattern, &relative))
        {
            matched.push(file.clone());
        }
    }
    matched
}

fn package_entry_candidates(directory: &Path, value: &str) -> Vec<PathBuf> {
    let value = value.strip_prefix("./").unwrap_or(value);
    if value.is_empty() || value.contains('*') || value.starts_with("http") {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    if let Some(source) = crate::resolve::source_directory_swap(directory, value) {
        candidates.push(source);
    }
    candidates.push(directory.join(value));
    candidates
}

fn index_html_entries(directory: &Path, root: &Path, entries: &mut HashSet<PathBuf>) {
    let Ok(text) = fs::read_to_string(directory.join("index.html")) else {
        return;
    };
    for source in script_sources(&text) {
        let source = source.trim();
        if source.is_empty()
            || source.starts_with("http://")
            || source.starts_with("https://")
            || source.starts_with("//")
            || source.starts_with("data:")
        {
            continue;
        }
        let path = if let Some(relative) = source.strip_prefix('/') {
            root.join(relative)
        } else {
            directory.join(source)
        };
        insert_entry(entries, &path);
    }
}

fn script_sources(text: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("<script") {
        let after = &rest[index + "<script".len()..];
        let end = after.find('>').unwrap_or(after.len());
        let tag = &after[..end];
        for token in tag.split_ascii_whitespace() {
            if let Some(value) = token.strip_prefix("src=") {
                sources.push(
                    value
                        .trim_matches(|character| matches!(character, '"' | '\'' | '>'))
                        .to_owned(),
                );
            }
        }
        rest = &after[end..];
    }
    sources
}

fn is_entry_file(root: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(stem) = source_stem(name) else {
        return false;
    };
    // Framework and layout conventions are relative to the package, not to the
    // graph root. A workspace root keeps every package nested, so comparing
    // against it would treat package directory names such as `app` as route
    // folders.
    let project = project_directory(path, root);
    let relative = path.strip_prefix(&project).unwrap_or(path);
    let components: Vec<&str> = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    let directories = &components[..components.len().saturating_sub(1)];
    is_config_entry(stem, directories)
        || is_test_entry(name, directories)
        || is_script_entry(directories)
        || is_framework_entry(stem, name, directories)
}

/// The nearest package or tsconfig directory between `path` and the graph
/// root.
fn project_directory(path: &Path, root: &Path) -> PathBuf {
    let mut directory = path.parent();
    while let Some(current) = directory {
        if current.join("package.json").is_file()
            || current.join("tsconfig.json").is_file()
            || current.join("jsconfig.json").is_file()
        {
            return current.to_path_buf();
        }
        if current == root {
            break;
        }
        directory = current.parent();
    }
    root.to_path_buf()
}

fn source_stem(name: &str) -> Option<&str> {
    let (stem, extension) = name.rsplit_once('.')?;
    SOURCE_EXTENSIONS
        .contains(&extension.to_ascii_lowercase().as_str())
        .then_some(stem)
}

fn is_config_entry(stem: &str, directories: &[&str]) -> bool {
    if directories
        .iter()
        .any(|directory| matches!(*directory, ".storybook" | "storybook"))
    {
        return true;
    }
    stem.ends_with(".config")
        || stem == "knexfile"
        || stem.starts_with("knexfile.")
        || matches!(
            stem,
            ".eslintrc" | ".babelrc" | ".prettierrc" | "gulpfile" | "gruntfile" | "prettier"
        )
}

fn is_test_entry(name: &str, directories: &[&str]) -> bool {
    let name = name.to_ascii_lowercase();
    if [
        ".test.",
        ".spec.",
        ".story.",
        ".stories.",
        ".bench.",
        ".e2e.",
    ]
    .iter()
    .any(|marker| name.contains(marker))
    {
        return true;
    }
    directories.iter().any(|directory| {
        matches!(
            *directory,
            "__tests__"
                | "tests"
                | "test"
                | "e2e"
                | "cypress"
                | "playwright"
                | "storybook"
                | ".storybook"
                | "__mocks__"
                | "bench"
                | "benches"
                | "benchmarks"
        )
    })
}

fn is_script_entry(directories: &[&str]) -> bool {
    directories.iter().any(|directory| {
        matches!(
            *directory,
            "bin"
                | "scripts"
                | "script"
                | "migrations"
                | "migration"
                | "tools"
                | "tooling"
                | ".husky"
        )
    })
}

fn is_framework_entry(stem: &str, name: &str, directories: &[&str]) -> bool {
    let under_app = directories.contains(&"app");
    if under_app {
        if matches!(
            stem,
            "page"
                | "layout"
                | "route"
                | "template"
                | "loading"
                | "error"
                | "not-found"
                | "global-error"
                | "default"
                | "opengraph-image"
                | "twitter-image"
                | "icon"
                | "apple-icon"
                | "sitemap"
                | "robots"
                | "manifest"
        ) {
            return true;
        }
        if matches!(stem, "root" | "entry.client" | "entry.server") {
            return true;
        }
    }
    if directories
        .windows(2)
        .any(|window| window[0] == "app" && window[1] == "routes")
    {
        return true;
    }
    for (index, directory) in directories.iter().enumerate() {
        if *directory == "pages" && (index == 0 || (index == 1 && directories[0] == "src")) {
            return true;
        }
    }
    if ["+page.", "+layout.", "+server.", "+error."]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return true;
    }
    if directories.len() == 1
        && directories[0] == "src"
        && matches!(stem, "main" | "main.server" | "app.config" | "app.module")
    {
        return true;
    }
    if directories.is_empty() && matches!(stem, "main" | "index") {
        return true;
    }
    matches!(stem, "main" | "preload" | "renderer")
        && (directories.is_empty()
            || directories
                .iter()
                .any(|directory| matches!(*directory, "src" | "electron" | "app")))
}

static TSC_CACHE: OnceLock<TscCache> = OnceLock::new();

type DiagnosticList = Vec<(PathBuf, TypeDiagnostic)>;
type SharedDiagnostics = Arc<DiagnosticList>;

struct TscCache {
    diagnostics: Mutex<HashMap<TscCacheKey, SharedDiagnostics>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TscCacheKey {
    root: PathBuf,
    tsconfig: PathBuf,
    compiler: TscProgram,
    tsconfig_fingerprint: u64,
    source_fingerprint: u64,
    timeout: Duration,
}

fn cached_type_diagnostics(
    project_root: &Path,
    tsconfig: &Path,
    compiler: Option<&TscProgram>,
    timeout: Duration,
) -> Result<SharedDiagnostics, Diagnostic> {
    let root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let tsconfig = fs::canonicalize(tsconfig).unwrap_or_else(|_| tsconfig.to_path_buf());
    let compiler = compiler.cloned().unwrap_or_else(|| resolve_compiler(&root));
    let key = TscCacheKey {
        root: root.clone(),
        tsconfig: tsconfig.clone(),
        compiler,
        tsconfig_fingerprint: tsconfig_fingerprint(&tsconfig),
        source_fingerprint: source_fingerprint(&root),
        timeout,
    };
    let cache = TSC_CACHE.get_or_init(|| TscCache {
        diagnostics: Mutex::new(HashMap::new()),
    });
    // Holding the lock for the duration of the run coalesces concurrent
    // requests for the same project into one compiler invocation.
    let mut diagnostics = lock(&cache.diagnostics);
    if let Some(diagnostics) = diagnostics.get(&key).cloned() {
        return Ok(diagnostics);
    }
    let result = Arc::new(run_tsc(&root, &tsconfig, &key.compiler, timeout)?);
    diagnostics.retain(|cached, _| cached.root != key.root || cached.tsconfig != key.tsconfig);
    diagnostics.insert(key, Arc::clone(&result));
    Ok(result)
}

fn tsconfig_fingerprint(tsconfig: &Path) -> u64 {
    let config = crate::tsconfig::load_at(tsconfig);
    let mut hasher = DefaultHasher::new();
    if config.chain.is_empty() {
        hash_file(&mut hasher, tsconfig);
    } else {
        for file in &config.chain {
            hash_file(&mut hasher, file);
        }
    }
    hasher.finish()
}

fn source_fingerprint(root: &Path) -> u64 {
    fingerprint(&source_listing(root))
}

fn run_tsc(
    project_root: &Path,
    tsconfig: &Path,
    compiler: &TscProgram,
    timeout: Duration,
) -> Result<DiagnosticList, Diagnostic> {
    let mut child = Command::new(&compiler.program)
        .args(&compiler.arguments)
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false")
        .arg("--locale")
        .arg("en")
        .arg("--project")
        .arg(tsconfig)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Diagnostic::new(
                DiagnosticKind::Io,
                format!("failed to run tsc for {}: {error}", tsconfig.display()),
            )
        })?;
    // Drain both pipes concurrently: a compiler can emit more than a pipe
    // buffer before exiting, and waiting for it without reading would deadlock.
    let stdout = child.stdout.take().map(read_pipe);
    let stderr = child.stderr.take().map(read_pipe);
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // The readers are intentionally detached: a grandchild may
                // still hold the pipe, and joining would reintroduce the hang.
                let tsconfig =
                    fs::canonicalize(tsconfig).unwrap_or_else(|_| tsconfig.to_path_buf());
                return Ok(vec![(
                    tsconfig,
                    TypeDiagnostic {
                        code: TSC_TIMEOUT_CODE,
                        message: format!(
                            "TypeScript compiler timed out after {} ms",
                            timeout.as_millis()
                        ),
                        start: 0,
                        end: 0,
                    },
                )]);
            }
            Ok(None) => std::thread::sleep(TSC_TIMEOUT_POLL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Diagnostic::new(
                    DiagnosticKind::Io,
                    format!("failed to wait for tsc for {}: {error}", tsconfig.display()),
                ));
            }
        }
    }
    let mut text = stdout
        .and_then(|reader| reader.join().ok())
        .map_or_else(String::new, |bytes| {
            String::from_utf8_lossy(&bytes).into_owned()
        });
    if let Some(bytes) = stderr.and_then(|reader| reader.join().ok()) {
        text.push_str(&String::from_utf8_lossy(&bytes));
    }
    Ok(parse_tsc_output(project_root, tsconfig, &text))
}

fn read_pipe(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

#[derive(Debug)]
struct RawDiagnostic {
    file: Option<PathBuf>,
    line: usize,
    column: usize,
    code: u32,
    message: String,
}

fn parse_tsc_output(
    project_root: &Path,
    tsconfig: &Path,
    output: &str,
) -> Vec<(PathBuf, TypeDiagnostic)> {
    let mut parsed: Vec<RawDiagnostic> = Vec::new();
    for line in output.lines() {
        if let Some(diagnostic) = parse_diagnostic_line(line) {
            parsed.push(diagnostic);
        } else if line.starts_with("  ")
            && !line.trim().is_empty()
            && let Some(last) = parsed.last_mut()
        {
            if !last.message.is_empty() {
                last.message.push('\n');
            }
            last.message.push_str(line.trim());
        }
    }
    let tsconfig = fs::canonicalize(tsconfig).unwrap_or_else(|_| tsconfig.to_path_buf());
    let mut diagnostics = Vec::new();
    for raw in parsed {
        let positioned = raw.file.is_some();
        let file = raw.file.map_or_else(
            || tsconfig.clone(),
            |file| {
                if file.is_absolute() {
                    file
                } else {
                    project_root.join(file)
                }
            },
        );
        let file = fs::canonicalize(&file).unwrap_or(file);
        let (start, end) = if positioned {
            let Ok(source) = fs::read_to_string(&file) else {
                continue;
            };
            let source = source.strip_prefix('\u{feff}').unwrap_or(&source);
            let Some((start, end)) = span_for_line_column(source, raw.line, raw.column) else {
                continue;
            };
            (start, end)
        } else {
            (0, 0)
        };
        diagnostics.push((
            file,
            TypeDiagnostic {
                code: raw.code,
                message: raw.message,
                start,
                end,
            },
        ));
    }
    diagnostics.sort_by_key(|(file, diagnostic)| {
        (
            file.clone(),
            diagnostic.start,
            diagnostic.end,
            diagnostic.code,
        )
    });
    diagnostics.dedup();
    diagnostics
}

fn parse_diagnostic_line(line: &str) -> Option<RawDiagnostic> {
    let trimmed = line.trim_start();
    if let Some((head, code, message)) = split_severity(trimmed) {
        let (file, line_number, column) = parse_location(head)?;
        return Some(RawDiagnostic {
            file: Some(file),
            line: line_number,
            column,
            code,
            message,
        });
    }
    let rest = trimmed
        .strip_prefix("error TS")
        .or_else(|| trimmed.strip_prefix("warning TS"))?;
    let (code, message) = parse_code(rest)?;
    Some(RawDiagnostic {
        file: None,
        line: 0,
        column: 0,
        code,
        message,
    })
}

fn split_severity(line: &str) -> Option<(&str, u32, String)> {
    let (head, rest) = line
        .split_once(": error TS")
        .or_else(|| line.split_once(": warning TS"))?;
    let (code, message) = parse_code(rest)?;
    let head = head.trim_end();
    let head = head.strip_suffix(" -").unwrap_or(head);
    Some((head, code, message))
}

fn parse_code(rest: &str) -> Option<(u32, String)> {
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let code = digits.parse().ok()?;
    let message = rest[digits.len()..]
        .trim_start_matches([':', ' '])
        .to_owned();
    Some((code, message))
}

fn parse_location(head: &str) -> Option<(PathBuf, usize, usize)> {
    if let Some((file, coordinates)) = head.rsplit_once('(')
        && let Some(coordinates) = coordinates.strip_suffix(')')
    {
        let (line, column) = coordinates.split_once(',')?;
        return Some((
            PathBuf::from(file),
            line.parse().ok()?,
            column.parse().ok()?,
        ));
    }
    let mut parts = head.rsplitn(3, ':');
    let column = parts.next()?.parse().ok()?;
    let line = parts.next()?.parse().ok()?;
    let file = parts.next()?;
    Some((PathBuf::from(file), line, column))
}

fn span_for_line_column(source: &str, line: usize, column: usize) -> Option<(usize, usize)> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut offset = 0;
    let mut selected = None;
    for (index, text) in source.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            let body = text.trim_end_matches('\n').trim_end_matches('\r');
            let mut units = 0;
            let mut byte = body.len();
            for (index, character) in body.char_indices() {
                if units >= column - 1 {
                    byte = index;
                    break;
                }
                units += character.len_utf16();
            }
            selected = Some(offset + byte);
            break;
        }
        offset += text.len();
    }
    let start = selected?;
    let line_end = source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset);
    let rest = &source[start..line_end];
    let token_end = rest
        .char_indices()
        .find(|(_, character)| {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | ')'
                        | ','
                        | ';'
                        | ':'
                        | '='
                        | '{'
                        | '}'
                        | '['
                        | ']'
                        | '<'
                        | '>'
                        | '|'
                        | '&'
                        | '?'
                        | '!'
                        | '+'
                        | '-'
                        | '*'
                        | '/'
                        | '%'
                        | '.'
                        | '#'
                        | '@'
                        | '\''
                        | '"'
                        | '`'
                )
        })
        .map_or(rest.len(), |(offset, _)| offset);
    let end = if token_end == 0 {
        (start + 1).min(source.len())
    } else {
        start + token_end
    };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_marker_script(root: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join("fake-tsc");
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn tsc_output_is_parsed_to_byte_ranges_and_cached() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("a.ts"), "const x: number = \"s\";\n").unwrap();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let marker = root.join("calls");
        let script = write_marker_script(
            root,
            &format!(
                "echo call >> '{}'\necho 'a.ts(1,7): error TS2322: Type mismatch'",
                marker.display()
            ),
        );
        let tsconfig = root.join("tsconfig.json");
        clear_cache();
        let compiler = TscProgram {
            program: script,
            arguments: Vec::new(),
        };
        let diagnostics =
            cached_type_diagnostics(root, &tsconfig, Some(&compiler), TSC_TIMEOUT).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].1.code, 2322);
        assert_eq!(diagnostics[0].1.start, 6);
        assert_eq!(diagnostics[0].1.end, 7);

        let cached =
            cached_type_diagnostics(root, &tsconfig, Some(&compiler), TSC_TIMEOUT).unwrap();
        assert_eq!(cached, diagnostics);
        assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
    }

    #[test]
    fn parser_ignores_unrelated_lines() {
        let root = Path::new("/tmp/project");
        let output = "warning: something\n\n";
        assert!(parse_tsc_output(root, &root.join("tsconfig.json"), output).is_empty());
    }

    #[test]
    fn parser_keeps_positionless_and_continuation_lines() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("a.ts"), "const x: number = \"s\";\n").unwrap();
        let output = concat!(
            "error TS18003: No inputs were found in config file.\n",
            "  The config file has no input files.\n",
            "a.ts(1,7): error TS2322: Type 'string' is not assignable.\n",
            "  Types of property 'x' are incompatible.\n",
        );
        let diagnostics = parse_tsc_output(root, &root.join("tsconfig.json"), output);
        let positionless = diagnostics
            .iter()
            .find(|(_, diagnostic)| diagnostic.code == 18003)
            .expect("positionless diagnostic");
        assert!(positionless.1.message.contains("No inputs"));
        assert!(positionless.1.message.contains("no input files"));
        let positioned = diagnostics
            .iter()
            .find(|(_, diagnostic)| diagnostic.code == 2322)
            .expect("positioned diagnostic");
        assert!(
            positioned.1.message.contains("Types of property"),
            "continuation line should be kept: {}",
            positioned.1.message
        );
    }
}
