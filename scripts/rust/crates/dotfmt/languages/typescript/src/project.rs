//! Project-wide export usage analysis and TypeScript diagnostics.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::edit::{Analysis, Finding, Severity, Tag};
use ignore::{DirEntry, WalkBuilder};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier,
    Statement,
};
use oxc_parser::Parser;
use oxc_span::GetSpan;

use crate::TypeDiagnostic;
use crate::dialect::Dialect;

const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
const AVOIDED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".git",
    "coverage",
    "out",
    ".next",
    ".nuxt",
    "target",
];

/// Finds the project root by walking up to the nearest tsconfig/jsconfig or
/// package.json.
pub fn discover_root(start: &Path) -> Option<PathBuf> {
    let mut directory = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    let mut package = None;
    loop {
        if directory.join("tsconfig.json").is_file() || directory.join("jsconfig.json").is_file() {
            return Some(directory);
        }
        if package.is_none() && directory.join("package.json").is_file() {
            package = Some(directory.clone());
        }
        let Some(parent) = directory.parent() else {
            return package;
        };
        directory = parent.to_path_buf();
    }
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
        for finding in &mut analysis.findings {
            finding.start += '\u{feff}'.len_utf8();
            finding.end += '\u{feff}'.len_utf8();
        }
    }
    Ok(analysis)
}

pub(crate) fn unused_exports_for_analysis(path: &Path) -> Result<Analysis, Diagnostic> {
    unused_exports_stripped(path)
}

fn unused_exports_stripped(path: &Path) -> Result<Analysis, Diagnostic> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let Some(root) = discover_root(path).or_else(|| canonical.parent().map(Path::to_path_buf))
    else {
        return Ok(Analysis::default());
    };
    let root = fs::canonicalize(&root).unwrap_or(root);
    let graph = graph_for(&root)?;
    Ok(graph.analysis_for(&canonical))
}

/// Clears the cached project graphs and TypeScript diagnostics.
pub fn clear_cache() {
    if let Some(cache) = GRAPH_CACHE.get() {
        lock(cache).clear();
    }
    if let Some(cache) = TSC_CACHE.get() {
        lock(&cache.diagnostics).clear();
    }
}

/// Runs `tsc --noEmit` for `tsconfig` and parses TypeScript diagnostics.
///
/// Results are cached by project root and tsconfig modification time.
pub fn type_diagnostics(
    project_root: &Path,
    tsconfig: &Path,
) -> Result<Vec<TypeDiagnostic>, Diagnostic> {
    let diagnostics = cached_type_diagnostics(project_root, tsconfig, None, None)?;
    Ok(diagnostics
        .iter()
        .map(|(_, diagnostic)| diagnostic.clone())
        .collect())
}

pub(crate) fn type_diagnostics_for(
    path: &Path,
    project_root: &Path,
    tsconfig: &Path,
) -> Result<Vec<TypeDiagnostic>, Diagnostic> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let diagnostics = cached_type_diagnostics(project_root, tsconfig, None, Some(path))?;
    Ok(diagnostics
        .iter()
        .filter_map(|(file, diagnostic)| (file == &canonical).then_some(diagnostic.clone()))
        .collect())
}

#[derive(Clone, Debug)]
struct Graph {
    modules: HashMap<PathBuf, Module>,
    entries: HashSet<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct Module {
    exports: Vec<Export>,
    edges: Vec<Edge>,
    used_names: BTreeSet<String>,
    all_used: bool,
}

#[derive(Clone, Debug)]
struct Export {
    name: String,
    start: usize,
    end: usize,
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
}

impl Graph {
    fn analysis_for(&self, path: &Path) -> Analysis {
        let Some(module) = self.modules.get(path) else {
            return Analysis::default();
        };
        if self.entries.contains(path) || is_declaration_file(path) {
            return Analysis::default();
        }
        let mut analysis = Analysis::default();
        for export in &module.exports {
            if module.all_used || module.used_names.contains(&export.name) {
                continue;
            }
            analysis.findings.push(
                Finding::new(
                    "unused-export",
                    format!("Export `{}` is never imported in this project", export.name),
                    Severity::Warning,
                    export.start,
                    export.end,
                )
                .with_tag(Tag::Unnecessary),
            );
        }
        analysis
            .findings
            .sort_by_key(|finding| (finding.start, finding.end));
        analysis
    }
}

struct CachedGraph {
    fingerprint: u64,
    graph: Arc<Graph>,
}

type GraphCache = Mutex<HashMap<PathBuf, CachedGraph>>;

static GRAPH_CACHE: OnceLock<GraphCache> = OnceLock::new();

fn graph_for(root: &Path) -> Result<Arc<Graph>, Diagnostic> {
    let files = source_files(root);
    let fingerprint = fingerprint(&files);
    let cache = GRAPH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = lock(cache).get(root)
        && cached.fingerprint == fingerprint
    {
        return Ok(Arc::clone(&cached.graph));
    }
    let graph = Arc::new(build_graph(root, &files));
    lock(cache).insert(
        root.to_path_buf(),
        CachedGraph {
            fingerprint,
            graph: Arc::clone(&graph),
        },
    );
    Ok(graph)
}

fn fingerprint(files: &[PathBuf]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for path in files {
        path.hash(&mut hasher);
        if let Ok(metadata) = fs::metadata(path) {
            metadata.len().hash(&mut hasher);
            metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn build_graph(root: &Path, files: &[PathBuf]) -> Graph {
    let mut modules = HashMap::new();
    let entries = entry_points(root);
    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if text.len() > 4 * 1024 * 1024 {
            continue;
        }
        let module = parse_module(path, &text);
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        modules.insert(key, module);
    }

    let mut graph = Graph { modules, entries };
    graph.resolve_edges();
    graph
}

impl Graph {
    fn resolve_edges(&mut self) {
        let paths: Vec<PathBuf> = self.modules.keys().cloned().collect();
        let mut uses: HashMap<PathBuf, ModuleUses> = HashMap::new();
        for importer in paths {
            let Some(module) = self.modules.get(&importer) else {
                continue;
            };
            for edge in &module.edges {
                let Some(target) = resolve_import(&importer, &edge.specifier) else {
                    continue;
                };
                let target = fs::canonicalize(&target).unwrap_or(target);
                if !self.modules.contains_key(&target) {
                    continue;
                }
                let use_site = uses.entry(target).or_default();
                match &edge.kind {
                    EdgeKind::Named(names) => use_site.names.extend(names.iter().cloned()),
                    EdgeKind::Default => {
                        use_site.names.insert("default".to_owned());
                    }
                    EdgeKind::Namespace | EdgeKind::All => use_site.all = true,
                    EdgeKind::SideEffect => {}
                }
            }
        }
        for (path, use_site) in uses {
            if let Some(module) = self.modules.get_mut(&path) {
                module.all_used = use_site.all;
                module.used_names.extend(use_site.names);
            }
        }
    }
}

#[derive(Default)]
struct ModuleUses {
    names: BTreeSet<String>,
    all: bool,
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .follow_links(false)
        .require_git(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .filter_entry(|entry| !avoided(entry));
    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if is_source_file(entry.path()) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    files
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
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn is_declaration_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".d.ts"))
}

fn parse_module(path: &Path, text: &str) -> Module {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let source_type = Dialect::Auto.source_type_for_path(path);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Module::default();
    }
    let mut module = Module::default();
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
                export_declaration(&declaration.declaration, &mut module.exports);
            }
            Statement::ExportNamedDeclaration(declaration) => {
                for specifier in &declaration.specifiers {
                    module.exports.push(Export {
                        name: specifier.exported.name().as_str().to_owned(),
                        start: specifier.exported.span().start as usize,
                        end: specifier.exported.span().end as usize,
                    });
                }
            }
            Statement::ExportDefaultDeclaration(declaration)
                if !matches!(
                    declaration.declaration,
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
                ) =>
            {
                let start = declaration.span.start as usize;
                let end = declaration.span.end as usize;
                let offset = text
                    .get(start..end)
                    .and_then(|text| text.find("default"))
                    .map_or(start, |offset| start + offset);
                module.exports.push(Export {
                    name: "default".to_owned(),
                    start: offset,
                    end: offset + "default".len(),
                });
            }
            _ => {}
        }
    }
    module
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

fn export_declaration(declaration: &Declaration<'_>, exports: &mut Vec<Export>) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                collect_binding_exports(&declarator.id, exports);
            }
        }
        _ => {
            if let Some(identifier) = declaration.id() {
                exports.push(Export {
                    name: identifier.name.as_str().to_owned(),
                    start: identifier.span.start as usize,
                    end: identifier.span.end as usize,
                });
            }
        }
    }
}

fn collect_binding_exports(pattern: &BindingPattern<'_>, exports: &mut Vec<Export>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => exports.push(Export {
            name: identifier.name.as_str().to_owned(),
            start: identifier.span.start as usize,
            end: identifier.span.end as usize,
        }),
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_binding_exports(&property.value, exports);
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_exports(&rest.argument, exports);
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_binding_exports(element, exports);
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_exports(&rest.argument, exports);
            }
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_binding_exports(&pattern.left, exports);
        }
    }
}

fn resolve_import(importer: &Path, specifier: &str) -> Option<PathBuf> {
    if !specifier.starts_with('.') {
        return None;
    }
    let base = normalize(&importer.parent()?.join(specifier));
    if base.is_file() {
        return Some(base);
    }
    let mut candidates = Vec::new();
    if let Some(extension) = base.extension().and_then(|extension| extension.to_str()) {
        let without = base.with_extension("");
        match extension {
            "js" => {
                candidates.extend(["ts", "tsx"].map(|extension| without.with_extension(extension)))
            }
            "jsx" => candidates.push(without.with_extension("tsx")),
            "mjs" => candidates.push(without.with_extension("mts")),
            "cjs" => candidates.push(without.with_extension("cts")),
            _ => {}
        }
    } else {
        for extension in SOURCE_EXTENSIONS {
            candidates.push(base.with_extension(extension));
        }
        for extension in SOURCE_EXTENSIONS {
            candidates.push(base.join(format!("index.{extension}")));
        }
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn entry_points(root: &Path) -> HashSet<PathBuf> {
    let mut entries = HashSet::new();
    for extension in SOURCE_EXTENSIONS {
        if let Ok(path) = fs::canonicalize(root.join("src").join(format!("index.{extension}"))) {
            entries.insert(path);
        }
        if let Ok(path) = fs::canonicalize(root.join(format!("index.{extension}"))) {
            entries.insert(path);
        }
    }
    let Ok(package) = fs::read_to_string(root.join("package.json")) else {
        return entries;
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&package) else {
        return entries;
    };
    let mut values = Vec::new();
    for key in ["main", "module", "types", "typings"] {
        if let Some(value) = package.get(key).and_then(serde_json::Value::as_str) {
            values.push(value.to_owned());
        }
    }
    collect_export_strings(package.get("exports"), &mut values);
    for value in values {
        let value = value.strip_prefix("./").unwrap_or(&value);
        let path = root.join(value);
        for candidate in entry_candidates(&path) {
            if let Ok(candidate) = fs::canonicalize(candidate) {
                entries.insert(candidate);
            }
        }
    }
    entries
}

fn entry_candidates(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf()];
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("js") => {
            let without = path.with_extension("");
            candidates.extend(["ts", "tsx"].map(|extension| without.with_extension(extension)));
        }
        Some("jsx") => candidates.push(path.with_extension("tsx")),
        Some("mjs") => candidates.push(path.with_extension("mts")),
        Some("cjs") => candidates.push(path.with_extension("cts")),
        None => {
            for extension in SOURCE_EXTENSIONS {
                candidates.push(path.with_extension(extension));
            }
            for extension in SOURCE_EXTENSIONS {
                candidates.push(path.join(format!("index.{extension}")));
            }
        }
        _ => {}
    }
    candidates
}

fn collect_export_strings(value: Option<&serde_json::Value>, values: &mut Vec<String>) {
    match value {
        Some(serde_json::Value::String(value)) => values.push(value.clone()),
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                collect_export_strings(Some(item), values);
            }
        }
        Some(serde_json::Value::Object(map)) => {
            for value in map.values() {
                collect_export_strings(Some(value), values);
            }
        }
        _ => {}
    }
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
    compiler: Option<PathBuf>,
    modified: Option<SystemTime>,
    source_modified: Option<SystemTime>,
}

fn cached_type_diagnostics(
    project_root: &Path,
    tsconfig: &Path,
    compiler: Option<&Path>,
    source: Option<&Path>,
) -> Result<SharedDiagnostics, Diagnostic> {
    let root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let tsconfig = fs::canonicalize(tsconfig).unwrap_or_else(|_| tsconfig.to_path_buf());
    let modified = fs::metadata(&tsconfig)
        .and_then(|metadata| metadata.modified())
        .ok();
    let source_modified = source
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok());
    let key = TscCacheKey {
        root: root.clone(),
        tsconfig: tsconfig.clone(),
        compiler: compiler.map(Path::to_path_buf),
        modified,
        source_modified,
    };
    let cache = TSC_CACHE.get_or_init(|| TscCache {
        diagnostics: Mutex::new(HashMap::new()),
    });
    if let Some(diagnostics) = lock(&cache.diagnostics).get(&key).cloned() {
        return Ok(diagnostics);
    }
    let diagnostics = Arc::new(run_tsc(&root, &tsconfig, compiler)?);
    lock(&cache.diagnostics).insert(key, Arc::clone(&diagnostics));
    Ok(diagnostics)
}

fn run_tsc(
    project_root: &Path,
    tsconfig: &Path,
    compiler: Option<&Path>,
) -> Result<DiagnosticList, Diagnostic> {
    let tsc = compiler
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("DOTFMT_TSC").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("tsc"));
    let output = Command::new(tsc)
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false")
        .arg("--project")
        .arg(tsconfig)
        .current_dir(project_root)
        .output()
        .map_err(|error| {
            Diagnostic::new(
                DiagnosticKind::Io,
                format!("failed to run tsc for {}: {error}", tsconfig.display()),
            )
        })?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(parse_tsc_output(project_root, &text))
}

fn parse_tsc_output(project_root: &Path, output: &str) -> Vec<(PathBuf, TypeDiagnostic)> {
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        let Some((location, rest)) = line.split_once("): error TS") else {
            continue;
        };
        let Some((file, coordinates)) = location.rsplit_once('(') else {
            continue;
        };
        let Some((line_number, column)) = coordinates.split_once(',') else {
            continue;
        };
        let Ok(line_number) = line_number.parse::<usize>() else {
            continue;
        };
        let Ok(column) = column.parse::<usize>() else {
            continue;
        };
        let code = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        let Ok(code) = code.parse::<u32>() else {
            continue;
        };
        let message = rest[code.to_string().len()..]
            .trim_start_matches([':', ' '])
            .to_owned();
        let file = PathBuf::from(file);
        let file = if file.is_absolute() {
            file
        } else {
            project_root.join(file)
        };
        let file = fs::canonicalize(&file).unwrap_or(file);
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        let source = source.strip_prefix('\u{feff}').unwrap_or(&source);
        let Some((start, end)) = span_for_line_column(source, line_number, column) else {
            continue;
        };
        diagnostics.push((
            file,
            TypeDiagnostic {
                code,
                message,
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
        let diagnostics = cached_type_diagnostics(root, &tsconfig, Some(&script), None).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].1.code, 2322);
        assert_eq!(diagnostics[0].1.start, 6);
        assert_eq!(diagnostics[0].1.end, 7);

        let cached = cached_type_diagnostics(root, &tsconfig, Some(&script), None).unwrap();
        assert_eq!(cached, diagnostics);
        assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
    }

    #[test]
    fn parser_ignores_unrelated_lines() {
        let root = Path::new("/tmp/project");
        let output = "warning: something\n\n";
        assert!(parse_tsc_output(root, output).is_empty());
    }
}
