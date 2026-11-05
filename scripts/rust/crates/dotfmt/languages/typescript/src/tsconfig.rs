use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_EXTENDS_DEPTH: usize = 8;
const MAX_REFERENCE_DEPTH: usize = 8;

#[derive(Clone, Debug, Default)]
pub(crate) struct Tsconfig {
    pub missing: bool,
    pub invalid: bool,
    pub jsx: Option<String>,
    pub jsx_factory: Option<String>,
    pub jsx_fragment_factory: Option<String>,
    pub verbatim_module_syntax: bool,
    pub imports_not_used_as_values: Option<String>,
    pub preserve_value_imports: bool,
    pub emit_decorator_metadata: bool,
    #[allow(dead_code)]
    pub experimental_decorators: bool,
    pub no_unused_locals: Option<bool>,
    pub no_unused_parameters: Option<bool>,
    pub base_url: Option<PathBuf>,
    pub paths: BTreeMap<String, Vec<String>>,
    pub paths_directory: Option<PathBuf>,
    pub directory: PathBuf,
    pub references: Vec<PathBuf>,
    pub solution: bool,
    #[allow(dead_code)]
    pub include: Vec<String>,
    #[allow(dead_code)]
    pub exclude: Vec<String>,
    #[allow(dead_code)]
    pub files: Vec<PathBuf>,
    #[allow(dead_code)]
    pub composite: bool,
    pub chain: Vec<PathBuf>,
}

impl Tsconfig {
    pub(crate) fn elides_unused_imports(&self) -> bool {
        !self.verbatim_module_syntax
            && !self.preserve_value_imports
            && !matches!(
                self.imports_not_used_as_values.as_deref(),
                Some("preserve" | "error")
            )
    }

    pub(crate) fn allows_type_only_import_conversion(&self) -> bool {
        if self.verbatim_module_syntax
            || self.preserve_value_imports
            || self.emit_decorator_metadata
        {
            return false;
        }
        !matches!(
            self.imports_not_used_as_values.as_deref(),
            Some("preserve" | "error")
        )
    }

    pub(crate) fn classic_jsx(&self) -> bool {
        self.missing || matches!(self.jsx.as_deref(), Some("react" | "preserve"))
    }

    pub(crate) fn protected_jsx_names(&self) -> Vec<String> {
        if !self.classic_jsx() {
            return Vec::new();
        }
        let mut names = Vec::new();
        for factory in [
            self.jsx_factory.as_deref().unwrap_or("React.createElement"),
            self.jsx_fragment_factory
                .as_deref()
                .unwrap_or("React.Fragment"),
        ] {
            if let Some(root) = factory.split('.').next()
                && !root.is_empty()
            {
                names.push(root.to_owned());
            }
        }
        names
    }
}

pub(crate) fn configs_in(directory: &Path) -> Vec<PathBuf> {
    let tsconfig = directory.join("tsconfig.json");
    if tsconfig.is_file() {
        return vec![tsconfig];
    }
    let jsconfig = directory.join("jsconfig.json");
    if jsconfig.is_file() {
        return vec![jsconfig];
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut configs: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .into_string()
                .ok()
                .map(|name| (name, entry))
        })
        .filter(|(name, _)| is_nonstandard_config_name(name))
        .map(|(_, entry)| entry.path())
        .filter(|path| path.is_file())
        .collect();
    configs.sort();
    configs
}

pub(crate) fn is_nonstandard_config_name(name: &str) -> bool {
    name.ends_with(".json")
        && (name.starts_with("tsconfig") || name.starts_with("jsconfig"))
        && name != "tsconfig.json"
        && name != "jsconfig.json"
}

pub(crate) fn is_config_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name == "tsconfig.json" || name == "jsconfig.json" || is_nonstandard_config_name(name)
        })
}

pub(crate) fn nearest_configs(path: &Path) -> Vec<PathBuf> {
    let Some(mut directory) = (if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    }) else {
        return Vec::new();
    };
    loop {
        let configs = configs_in(directory);
        if !configs.is_empty() {
            return configs;
        }
        let Some(parent) = directory.parent() else {
            return Vec::new();
        };
        directory = parent;
    }
}

pub(crate) fn nearest(path: &Path) -> Option<PathBuf> {
    nearest_configs(path).into_iter().next()
}

pub(crate) fn nearest_configs_within(path: &Path, root: &Path) -> Vec<PathBuf> {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let Some(start) = (if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    }) else {
        return Vec::new();
    };
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    if !start.starts_with(&root) {
        return Vec::new();
    }
    let mut directory = start.as_path();
    loop {
        let configs = configs_in(directory);
        if !configs.is_empty() {
            return configs;
        }
        if directory == root {
            return Vec::new();
        }
        let Some(parent) = directory.parent() else {
            return Vec::new();
        };
        directory = parent;
    }
}

pub(crate) fn nearest_within(path: &Path, root: &Path) -> Option<PathBuf> {
    nearest_configs_within(path, root).into_iter().next()
}

pub(crate) fn load_for(path: &Path) -> Tsconfig {
    nearest(path).map_or_else(|| missing_for(path), |path| load_config(&path))
}

pub(crate) fn load_for_within(path: &Path, root: &Path) -> Tsconfig {
    nearest_within(path, root).map_or_else(|| missing_for(path), |path| load_config(&path))
}

pub(crate) fn load_at(path: &Path) -> Tsconfig {
    load_config(path)
}

pub(crate) fn is_solution_config_file(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Some(jsonc_parser::JsonValue::Object(object)) =
        jsonc_parser::parse_to_value(&text, &Default::default())
            .ok()
            .flatten()
    else {
        return false;
    };
    object
        .get_array("files")
        .is_some_and(|files| files.is_empty())
        && object
            .get_array("references")
            .is_some_and(|references| !references.is_empty())
}

pub(crate) fn resolution_configs_for(path: &Path, root: &Path) -> Vec<Tsconfig> {
    match nearest_within(path, root) {
        Some(path) => resolution_configs(&path),
        None => vec![load_for_within(path, root)],
    }
}

fn resolution_configs(path: &Path) -> Vec<Tsconfig> {
    let config = load_at(path);
    if !config.solution {
        return vec![config];
    }
    let mut configs = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    collect_resolution_configs(config, 0, &mut visited, &mut configs);
    configs
}

fn collect_resolution_configs(
    config: Tsconfig,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    configs: &mut Vec<Tsconfig>,
) {
    let references = config.references.clone();
    let index = configs.len();
    configs.push(config);
    if depth >= MAX_REFERENCE_DEPTH {
        configs[index].invalid = true;
        return;
    }
    for reference in references {
        let Some(path) = resolve_config_path(&reference) else {
            configs[index].invalid = true;
            continue;
        };
        let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !visited.insert(key) {
            continue;
        }
        let referenced = load_at(&path);
        if referenced.missing {
            configs[index].invalid = true;
            continue;
        }
        if referenced.invalid {
            configs[index].invalid = true;
        }
        collect_resolution_configs(referenced, depth + 1, visited, configs);
    }
}

fn load_config(path: &Path) -> Tsconfig {
    let directory = path.parent().unwrap_or(Path::new("."));
    let mut invalid = false;
    load_file(path, 0, &mut Vec::new(), &mut invalid).map_or_else(
        || Tsconfig {
            missing: true,
            invalid: true,
            directory: directory.to_path_buf(),
            ..Tsconfig::default()
        },
        |raw| {
            let mut config = raw.into_config(directory);
            config.invalid = invalid;
            config
        },
    )
}

fn missing_for(path: &Path) -> Tsconfig {
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(Path::new("."))
    };
    Tsconfig {
        missing: true,
        directory: directory.to_path_buf(),
        ..Tsconfig::default()
    }
}

fn load_file(
    path: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    invalid: &mut bool,
) -> Option<Raw> {
    if depth > MAX_EXTENDS_DEPTH {
        *invalid = true;
        return None;
    }
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if stack.contains(&key) {
        *invalid = true;
        return None;
    }
    stack.push(key);
    let result = load_file_inner(path, depth, stack, invalid);
    stack.pop();
    result
}

fn load_file_inner(
    path: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    invalid: &mut bool,
) -> Option<Raw> {
    let Ok(text) = fs::read_to_string(path) else {
        *invalid = true;
        return None;
    };
    let value = match jsonc_parser::parse_to_value(&text, &Default::default()) {
        Ok(Some(value)) => value,
        Ok(None) => {
            let mut raw = Raw::default();
            raw.chain
                .push(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
            return Some(raw);
        }
        Err(_) => {
            *invalid = true;
            return None;
        }
    };
    let jsonc_parser::JsonValue::Object(object) = value else {
        *invalid = true;
        return None;
    };
    let directory = path.parent().unwrap_or(Path::new("."));

    let mut raw = Raw::default();
    for extends in extends_values(&object) {
        let Some(extended) = resolve_extends(directory, &extends) else {
            *invalid = true;
            continue;
        };
        if let Some(parent) = load_file(&extended, depth + 1, stack, invalid) {
            raw.inherit(parent);
        }
    }
    if let Some(options) = object.get_object("compilerOptions") {
        raw.apply(options, directory);
    }
    raw.apply_project_fields(&object, directory);
    raw.chain
        .push(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));

    Some(raw)
}

fn extends_values(object: &jsonc_parser::JsonObject<'_>) -> Vec<String> {
    if let Some(extends) = object.get_string("extends") {
        return vec![extends.to_string()];
    }
    let Some(extends) = object.get_array("extends") else {
        return Vec::new();
    };
    extends
        .iter()
        .filter_map(|value| match value {
            jsonc_parser::JsonValue::String(value) => Some(value.to_string()),
            _ => None,
        })
        .collect()
}

fn is_relative(value: &str) -> bool {
    value.starts_with("./") || value.starts_with("../")
}

fn resolve_extends(directory: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() {
        resolve_config_path(path)
    } else if is_relative(value) {
        resolve_config_path(&directory.join(path))
    } else {
        resolve_package(directory, value)
    }
}

fn resolve_config_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.extension().is_none() {
        let json = path.with_extension("json");
        if json.is_file() {
            return Some(json);
        }
    }
    if path.is_dir() {
        let tsconfig = path.join("tsconfig.json");
        if tsconfig.is_file() {
            return Some(tsconfig);
        }
    }
    None
}

fn resolve_package(directory: &Path, specifier: &str) -> Option<PathBuf> {
    let (name, subpath) = split_package(specifier)?;
    for ancestor in directory.ancestors() {
        let root = ancestor.join("node_modules").join(&name);
        if !root.is_dir() {
            continue;
        }
        if !subpath.is_empty() {
            if let Some(path) = resolve_config_path(&root.join(subpath)) {
                return Some(path);
            }
            continue;
        }
        if let Some(path) = package_tsconfig(&root) {
            return Some(path);
        }
        if let Some(path) = resolve_config_path(&root) {
            return Some(path);
        }
    }
    None
}

fn split_package(specifier: &str) -> Option<(String, &str)> {
    if specifier.starts_with('@') {
        let scope_end = specifier.find('/')?;
        let rest = &specifier[scope_end + 1..];
        let (name, subpath) = rest.split_once('/').unwrap_or((rest, ""));
        if name.is_empty() {
            return None;
        }
        return Some((format!("{}/{name}", &specifier[..scope_end]), subpath));
    }
    let (name, subpath) = specifier.split_once('/').unwrap_or((specifier, ""));
    (!name.is_empty()).then(|| (name.to_owned(), subpath))
}

fn package_tsconfig(root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(root.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let tsconfig = value.get("tsconfig")?.as_str()?;
    resolve_config_path(&root.join(tsconfig))
}

#[derive(Clone, Debug)]
struct RawPath {
    directory: PathBuf,
    value: String,
}

#[derive(Clone, Debug)]
struct RawList {
    directory: PathBuf,
    values: Vec<String>,
}

#[derive(Clone, Debug)]
struct RawPaths {
    directory: PathBuf,
    values: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Default)]
struct Raw {
    jsx: Option<String>,
    jsx_factory: Option<String>,
    jsx_fragment_factory: Option<String>,
    verbatim_module_syntax: Option<bool>,
    imports_not_used_as_values: Option<String>,
    preserve_value_imports: Option<bool>,
    emit_decorator_metadata: Option<bool>,
    experimental_decorators: Option<bool>,
    no_unused_locals: Option<bool>,
    no_unused_parameters: Option<bool>,
    base_url: Option<RawPath>,
    paths: Option<RawPaths>,
    references: Option<RawList>,
    include: Option<RawList>,
    exclude: Option<RawList>,
    files: Option<RawList>,
    composite: Option<bool>,
    chain: Vec<PathBuf>,
}

impl Raw {
    fn inherit(&mut self, parent: Self) {
        let Self {
            jsx,
            jsx_factory,
            jsx_fragment_factory,
            verbatim_module_syntax,
            imports_not_used_as_values,
            preserve_value_imports,
            emit_decorator_metadata,
            experimental_decorators,
            no_unused_locals,
            no_unused_parameters,
            base_url,
            paths,
            references,
            include,
            exclude,
            files,
            composite,
            chain,
        } = parent;
        merge_option(&mut self.jsx, jsx);
        merge_option(&mut self.jsx_factory, jsx_factory);
        merge_option(&mut self.jsx_fragment_factory, jsx_fragment_factory);
        merge_option(&mut self.verbatim_module_syntax, verbatim_module_syntax);
        merge_option(
            &mut self.imports_not_used_as_values,
            imports_not_used_as_values,
        );
        merge_option(&mut self.preserve_value_imports, preserve_value_imports);
        merge_option(&mut self.emit_decorator_metadata, emit_decorator_metadata);
        merge_option(&mut self.experimental_decorators, experimental_decorators);
        merge_option(&mut self.no_unused_locals, no_unused_locals);
        merge_option(&mut self.no_unused_parameters, no_unused_parameters);
        merge_option(&mut self.base_url, base_url);
        merge_option(&mut self.paths, paths);
        merge_option(&mut self.references, references);
        merge_option(&mut self.include, include);
        merge_option(&mut self.exclude, exclude);
        merge_option(&mut self.files, files);
        merge_option(&mut self.composite, composite);
        self.chain.extend(chain);
    }

    fn apply(&mut self, options: &jsonc_parser::JsonObject<'_>, directory: &Path) {
        if let Some(value) = options.get_string("jsx") {
            self.jsx = Some(value.as_ref().to_owned());
        }
        if let Some(value) = options.get_string("jsxFactory") {
            self.jsx_factory = Some(value.as_ref().to_owned());
        }
        if let Some(value) = options.get_string("jsxFragmentFactory") {
            self.jsx_fragment_factory = Some(value.as_ref().to_owned());
        }
        for (name, slot) in [
            ("verbatimModuleSyntax", &mut self.verbatim_module_syntax),
            ("preserveValueImports", &mut self.preserve_value_imports),
            ("emitDecoratorMetadata", &mut self.emit_decorator_metadata),
            ("experimentalDecorators", &mut self.experimental_decorators),
            ("noUnusedLocals", &mut self.no_unused_locals),
            ("noUnusedParameters", &mut self.no_unused_parameters),
            ("composite", &mut self.composite),
        ] {
            if let Some(value) = options.get_boolean(name) {
                *slot = Some(value);
            }
        }
        if let Some(value) = options.get_string("importsNotUsedAsValues") {
            self.imports_not_used_as_values = Some(value.as_ref().to_owned());
        }
        if let Some(value) = options.get_string("baseUrl") {
            self.base_url = Some(RawPath {
                directory: directory.to_path_buf(),
                value: value.to_string(),
            });
        }
        if let Some(paths) = options.get_object("paths") {
            let mut map = BTreeMap::new();
            for (key, value) in paths.clone().into_iter() {
                let jsonc_parser::JsonValue::Array(values) = value else {
                    continue;
                };
                map.insert(
                    key.into_owned(),
                    values
                        .iter()
                        .filter_map(|value| match value {
                            jsonc_parser::JsonValue::String(value) => Some(value.to_string()),
                            _ => None,
                        })
                        .collect(),
                );
            }
            self.paths = Some(RawPaths {
                directory: directory.to_path_buf(),
                values: map,
            });
        }
    }

    fn apply_project_fields(&mut self, object: &jsonc_parser::JsonObject<'_>, directory: &Path) {
        if let Some(references) = object.get_array("references") {
            let values = references
                .iter()
                .filter_map(|value| match value {
                    jsonc_parser::JsonValue::Object(reference) => {
                        reference.get_string("path").map(|path| path.to_string())
                    }
                    jsonc_parser::JsonValue::String(path) => Some(path.to_string()),
                    _ => None,
                })
                .collect();
            self.references = Some(RawList {
                directory: directory.to_path_buf(),
                values,
            });
        }
        for (name, slot) in [
            ("include", &mut self.include),
            ("exclude", &mut self.exclude),
            ("files", &mut self.files),
        ] {
            if let Some(array) = object.get_array(name) {
                *slot = Some(RawList {
                    directory: directory.to_path_buf(),
                    values: array
                        .iter()
                        .filter_map(|value| match value {
                            jsonc_parser::JsonValue::String(value) => Some(value.to_string()),
                            _ => None,
                        })
                        .collect(),
                });
            }
        }
    }

    fn into_config(self, config_dir: &Path) -> Tsconfig {
        let solution = self
            .files
            .as_ref()
            .is_some_and(|files| files.values.is_empty())
            && self
                .references
                .as_ref()
                .is_some_and(|references| !references.values.is_empty());
        let base_url = self
            .base_url
            .map(|raw| expand_path(&raw.directory, &raw.value, config_dir));
        let (paths, paths_directory) = self.paths.map_or_else(
            || (BTreeMap::new(), None),
            |raw| {
                let values = raw
                    .values
                    .into_iter()
                    .map(|(key, targets)| {
                        (
                            key,
                            targets
                                .into_iter()
                                .map(|target| expand_template(&target, config_dir))
                                .collect(),
                        )
                    })
                    .collect();
                (values, Some(raw.directory))
            },
        );
        let mut chain = self.chain;
        chain.sort();
        chain.dedup();
        Tsconfig {
            missing: false,
            invalid: false,
            jsx: self.jsx,
            jsx_factory: self.jsx_factory,
            jsx_fragment_factory: self.jsx_fragment_factory,
            verbatim_module_syntax: self.verbatim_module_syntax.unwrap_or(false),
            imports_not_used_as_values: self.imports_not_used_as_values,
            preserve_value_imports: self.preserve_value_imports.unwrap_or(false),
            emit_decorator_metadata: self.emit_decorator_metadata.unwrap_or(false),
            experimental_decorators: self.experimental_decorators.unwrap_or(false),
            no_unused_locals: self.no_unused_locals,
            no_unused_parameters: self.no_unused_parameters,
            base_url,
            paths,
            paths_directory,
            directory: config_dir.to_path_buf(),
            references: self.references.map_or_else(Vec::new, |raw| {
                raw.values
                    .iter()
                    .map(|value| expand_path(&raw.directory, value, config_dir))
                    .collect()
            }),
            solution,
            include: self
                .include
                .map_or_else(Vec::new, |raw| expand_patterns(&raw, config_dir)),
            exclude: self
                .exclude
                .map_or_else(Vec::new, |raw| expand_patterns(&raw, config_dir)),
            files: self.files.map_or_else(Vec::new, |raw| {
                raw.values
                    .iter()
                    .map(|value| expand_path(&raw.directory, value, config_dir))
                    .collect()
            }),
            composite: self.composite.unwrap_or(false),
            chain,
        }
    }
}

fn expand_template(value: &str, config_dir: &Path) -> String {
    if value.contains("${configDir}") {
        value.replace("${configDir}", &config_dir.to_string_lossy())
    } else {
        value.to_owned()
    }
}

fn expand_path(directory: &Path, value: &str, config_dir: &Path) -> PathBuf {
    let expanded = expand_template(value, config_dir);
    let path = Path::new(&expanded);
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&directory.join(path))
    }
}

fn expand_patterns(raw: &RawList, config_dir: &Path) -> Vec<String> {
    raw.values
        .iter()
        .map(|value| {
            let expanded = expand_template(value, config_dir);
            let path = Path::new(&expanded);
            if path.is_absolute() {
                path.to_string_lossy().into_owned()
            } else {
                raw.directory.join(path).to_string_lossy().into_owned()
            }
        })
        .collect()
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

fn merge_option<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

pub(crate) fn import_patterns(path: &Path) -> Option<Vec<String>> {
    let config = load_for(path);
    if config.paths.is_empty() {
        return None;
    }
    let mut patterns = Vec::new();
    for alias in config.paths.keys() {
        let prefix = alias.strip_suffix('*').unwrap_or(alias);
        let prefix = prefix.trim_end_matches('/');
        if prefix.is_empty() {
            continue;
        }
        let pattern = format!("{prefix}/");
        if !patterns.contains(&pattern) {
            patterns.push(pattern);
        }
    }
    (!patterns.is_empty()).then_some(patterns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn load(root: &Path) -> Tsconfig {
        let source = root.join("src/a.ts");
        if let Some(parent) = source.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(source, "").unwrap();
        load_for(&root.join("src/a.ts"))
    }

    #[test]
    fn extends_array_merges_in_order_and_skips_missing_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let absolute = tempfile::tempdir().unwrap();
        write(
            absolute.path(),
            "absolute.json",
            r#"{"compilerOptions":{"jsxFragmentFactory":"Fragment"}}"#,
        );
        write(
            root,
            "base.json",
            r#"{"compilerOptions":{"jsx":"react","jsxFactory":"Base","verbatimModuleSyntax":true}}"#,
        );
        write(
            root,
            "override.json",
            r#"{"compilerOptions":{"jsxFactory":"Override","noUnusedLocals":false}}"#,
        );
        write(
            root,
            "tsconfig.json",
            &format!(
                r#"{{"extends":["./base.json","./missing.json",{},"./override.json"],"compilerOptions":{{"noUnusedParameters":false}}}}"#,
                serde_json::to_string(absolute.path().join("absolute.json").to_str().unwrap())
                    .unwrap()
            ),
        );

        let config = load(root);
        assert!(!config.missing);
        assert_eq!(config.jsx.as_deref(), Some("react"));
        assert_eq!(config.jsx_factory.as_deref(), Some("Override"));
        assert_eq!(config.jsx_fragment_factory.as_deref(), Some("Fragment"));
        assert!(config.verbatim_module_syntax);
        assert_eq!(config.no_unused_locals, Some(false));
        assert_eq!(config.no_unused_parameters, Some(false));
    }

    #[test]
    fn malformed_configs_and_unresolved_extends_are_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } }"#,
        );
        write(root, "src/a.ts", "");
        let config = load_for(&root.join("src/a.ts"));
        assert!(config.invalid);
        assert!(
            config.missing,
            "an unreadable config keeps JSX conservative"
        );
        assert!(config.paths.is_empty());

        write(
            root,
            "tsconfig.json",
            r#"{"extends":"./missing-base.json","compilerOptions":{"baseUrl":"."}}"#,
        );
        let config = load_for(&root.join("src/a.ts"));
        assert!(config.invalid);
        assert!(!config.missing);
        assert!(config.base_url.is_some());
    }

    #[test]
    fn valid_configs_are_not_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
        let config = load(root);
        assert!(!config.invalid);
        assert!(!config.missing);

        write(root, "tsconfig.json", "");
        let config = load(root);
        assert!(!config.invalid);
        assert!(!config.missing);
    }

    #[test]
    fn broken_referenced_solution_configs_mark_the_solution_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"./packages/a"},{"path":"./packages/missing"}]}"#,
        );
        write(
            root,
            "packages/a/tsconfig.json",
            r#"{ "compilerOptions": {"#,
        );
        write(root, "src/a.ts", "");

        let configs = resolution_configs_for(&root.join("src/a.ts"), root);
        assert!(configs.iter().any(|config| config.invalid));
    }

    #[test]
    fn child_options_override_extended_options() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "base.json",
            r#"{"compilerOptions":{"jsx":"react","jsxFactory":"Base","verbatimModuleSyntax":true,"noUnusedLocals":true}}"#,
        );
        write(
            root,
            "tsconfig.json",
            r#"{"extends":"./base.json","compilerOptions":{"jsx":"react-jsx","verbatimModuleSyntax":false}}"#,
        );

        let config = load(root);
        assert_eq!(config.jsx.as_deref(), Some("react-jsx"));
        assert_eq!(config.jsx_factory.as_deref(), Some("Base"));
        assert!(!config.verbatim_module_syntax);
        assert_eq!(config.no_unused_locals, Some(true));
    }

    #[test]
    fn package_extends_resolve_through_node_modules_ancestry() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "node_modules/@tsconfig/node20/tsconfig.json",
            r#"{"compilerOptions":{"verbatimModuleSyntax":true}}"#,
        );
        write(
            root,
            "node_modules/some-package/tsconfig.json",
            r#"{"compilerOptions":{"jsxFactory":"Some"}}"#,
        );
        write(
            root,
            "node_modules/field-package/package.json",
            r#"{"tsconfig":"configs/base.json"}"#,
        );
        write(
            root,
            "node_modules/field-package/configs/base.json",
            r#"{"compilerOptions":{"jsxFragmentFactory":"Field"}}"#,
        );
        write(
            root,
            "node_modules/dir-package/tsconfig.json",
            r#"{"compilerOptions":{"noUnusedLocals":true}}"#,
        );
        write(
            root,
            "src/tsconfig.json",
            r#"{"extends":["@tsconfig/node20/tsconfig.json","some-package/tsconfig","field-package","dir-package"]}"#,
        );

        let config = load(root);
        assert!(!config.missing);
        assert!(config.verbatim_module_syntax);
        assert_eq!(config.jsx_factory.as_deref(), Some("Some"));
        assert_eq!(config.jsx_fragment_factory.as_deref(), Some("Field"));
        assert_eq!(config.no_unused_locals, Some(true));
    }

    #[test]
    fn extends_cycles_are_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "a.json",
            r#"{"extends":"./b.json","compilerOptions":{"jsx":"react"}}"#,
        );
        write(
            root,
            "b.json",
            r#"{"extends":"./a.json","compilerOptions":{"verbatimModuleSyntax":true}}"#,
        );
        write(root, "tsconfig.json", r#"{"extends":"./a.json"}"#);

        let config = load(root);
        assert_eq!(config.jsx.as_deref(), Some("react"));
        assert!(config.verbatim_module_syntax);
    }

    #[test]
    fn config_dir_placeholder_expands_against_the_root_config() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "configs/base.json",
            r#"{"compilerOptions":{"baseUrl":"${configDir}/src","paths":{"@/*":["${configDir}/src/*"]}}}"#,
        );
        write(
            root,
            "tsconfig.json",
            r#"{"extends":"./configs/base.json"}"#,
        );

        let config = load(root);
        assert_eq!(config.base_url.as_deref(), Some(root.join("src").as_path()));
        assert_eq!(
            config.paths.get("@/*").unwrap(),
            &vec![root.join("src/*").to_string_lossy().into_owned()]
        );
        assert_eq!(config.directory, root);
    }

    #[test]
    fn references_include_exclude_files_and_composite_are_parsed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "tsconfig.json",
            r#"{
                "compilerOptions":{"composite":true},
                "references":[{"path":"./packages/a"},{"path":"./packages/b"}],
                "include":["src/**/*"],
                "exclude":["src/generated"],
                "files":["src/main.ts"]
            }"#,
        );

        let config = load(root);
        assert!(config.composite);
        assert_eq!(
            config.references,
            vec![root.join("packages/a"), root.join("packages/b")]
        );
        assert_eq!(
            config.include,
            vec![root.join("src/**/*").to_string_lossy()]
        );
        assert_eq!(
            config.exclude,
            vec![root.join("src/generated").to_string_lossy()]
        );
        assert_eq!(config.files, vec![root.join("src/main.ts")]);
    }

    #[test]
    fn solution_configs_union_references_and_stop_at_cycles() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "a/tsconfig.json",
            r#"{"files":[],"references":[{"path":"../b"}]}"#,
        );
        write(
            root,
            "b/tsconfig.json",
            r#"{"files":[],"references":[{"path":"../c"}]}"#,
        );
        write(
            root,
            "c/tsconfig.json",
            r#"{"files":[],"references":[{"path":"../a"}],"compilerOptions":{"paths":{"@c/*":["src/*"]}}}"#,
        );
        write(root, "a/src/module.ts", "");

        let configs = resolution_configs_for(&root.join("a/src/module.ts"), &root.join("a"));
        assert_eq!(configs.len(), 3, "each solution config is visited once");
        assert!(configs.iter().any(|config| config.solution));
        assert!(
            configs
                .iter()
                .any(|config| config.paths.contains_key("@c/*"))
        );
    }

    #[test]
    fn normal_configs_apply_alone() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "tsconfig.json",
            r#"{"references":[{"path":"./packages/ui"}],"compilerOptions":{"baseUrl":"."}}"#,
        );
        write(
            root,
            "packages/ui/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@ui/*":["src/*"]}}}"#,
        );
        write(root, "src/module.ts", "");

        let configs = resolution_configs_for(&root.join("src/module.ts"), root);
        assert_eq!(configs.len(), 1);
        assert!(configs[0].paths.is_empty());
        assert!(configs[0].base_url.is_some());
    }

    #[test]
    fn nearest_within_stops_at_the_workspace_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
        write(root, "packages/app/src/a.ts", "");

        let source = root.join("packages/app/src/a.ts");
        assert_eq!(
            nearest_within(&source, root),
            Some(root.join("tsconfig.json"))
        );
        assert_eq!(nearest_within(&source, &root.join("packages/app")), None);
        assert_eq!(
            nearest(&source),
            Some(root.join("tsconfig.json")),
            "the unrestricted lookup still walks to the workspace root"
        );
    }

    #[test]
    fn configs_in_prefers_standard_names_then_all_nonstandard_sorted() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "tsconfig.router.json", "{}");
        write(root, "tsconfig.node.json", "{}");
        write(root, "jsconfig.legacy.json", "{}");
        write(root, "other.json", "{}");
        let names: Vec<PathBuf> = configs_in(root)
            .into_iter()
            .map(|path| path.file_name().unwrap().into())
            .collect();
        assert_eq!(
            names,
            vec![
                PathBuf::from("jsconfig.legacy.json"),
                PathBuf::from("tsconfig.node.json"),
                PathBuf::from("tsconfig.router.json"),
            ]
        );

        write(root, "tsconfig.json", "{}");
        assert_eq!(configs_in(root), vec![root.join("tsconfig.json")]);
        fs::remove_file(root.join("tsconfig.json")).unwrap();
        write(root, "jsconfig.json", "{}");
        assert_eq!(configs_in(root), vec![root.join("jsconfig.json")]);
        assert!(is_config_file(&root.join("tsconfig.router.json")));
        assert!(!is_config_file(&root.join("other.json")));
    }

    #[test]
    fn nearest_prefers_tsconfig_over_jsconfig() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "tsconfig.json", "{}");
        write(root, "jsconfig.json", "{}");
        write(root, "src/a.ts", "");
        assert_eq!(
            nearest(&root.join("src/a.ts")),
            Some(root.join("tsconfig.json"))
        );
    }
}
