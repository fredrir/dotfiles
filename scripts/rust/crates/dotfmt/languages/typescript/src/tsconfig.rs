use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_EXTENDS_DEPTH: usize = 8;

/// The tsconfig/jsconfig options that change the safety of a code fix.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tsconfig {
    /// No tsconfig/jsconfig was found; assume the classic JSX factory may be in
    /// scope rather than risk removing `React`/`h`.
    pub missing: bool,
    pub jsx: Option<String>,
    pub jsx_factory: Option<String>,
    pub jsx_fragment_factory: Option<String>,
    pub verbatim_module_syntax: bool,
    pub imports_not_used_as_values: Option<String>,
    pub preserve_value_imports: bool,
    pub emit_decorator_metadata: bool,
    /// Parsed for callers that gate decorator rewrites; no current fix reads it.
    #[allow(dead_code)]
    pub experimental_decorators: bool,
    pub no_unused_locals: Option<bool>,
    pub no_unused_parameters: Option<bool>,
    /// Parsed for path-alias resolution; import sorting uses `paths` directly.
    #[allow(dead_code)]
    pub base_url: Option<PathBuf>,
    pub paths: BTreeMap<String, Vec<String>>,
}

impl Tsconfig {
    /// Whether TypeScript's default emit drops unused imports entirely.
    pub(crate) fn elides_unused_imports(&self) -> bool {
        !self.verbatim_module_syntax
            && !self.preserve_value_imports
            && !matches!(
                self.imports_not_used_as_values.as_deref(),
                Some("preserve" | "error")
            )
    }

    /// A type-only import is behaviour-preserving only when TypeScript already
    /// elides the value import.
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

    /// The classic JSX transform reads its factory from an implicit binding.
    pub(crate) fn classic_jsx(&self) -> bool {
        self.missing || matches!(self.jsx.as_deref(), Some("react" | "preserve"))
    }

    /// Local names that must survive unused-import removal under the classic
    /// JSX transform (for example `React` or Preact's `h`).
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

/// Finds the nearest `tsconfig.json` or `jsconfig.json` for a source path.
pub(crate) fn nearest(path: &Path) -> Option<PathBuf> {
    let mut directory = if path.is_dir() { path } else { path.parent()? };
    loop {
        let tsconfig = directory.join("tsconfig.json");
        if tsconfig.is_file() {
            return Some(tsconfig);
        }
        let jsconfig = directory.join("jsconfig.json");
        if jsconfig.is_file() {
            return Some(jsconfig);
        }
        directory = directory.parent()?;
    }
}

/// Loads the nearest config, following relative and package `extends` chains.
pub(crate) fn load_for(path: &Path) -> Tsconfig {
    nearest(path)
        .and_then(|path| load_file(&path, 0, &mut Vec::new()))
        .map(Raw::into_config)
        .unwrap_or(Tsconfig {
            missing: true,
            ..Tsconfig::default()
        })
}

fn load_file(path: &Path, depth: usize, stack: &mut Vec<PathBuf>) -> Option<Raw> {
    if depth > MAX_EXTENDS_DEPTH {
        return None;
    }
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if stack.contains(&key) {
        return None;
    }
    stack.push(key);
    let result = load_file_inner(path, depth, stack);
    stack.pop();
    result
}

fn load_file_inner(path: &Path, depth: usize, stack: &mut Vec<PathBuf>) -> Option<Raw> {
    let text = fs::read_to_string(path).ok()?;
    let value = jsonc_parser::parse_to_value(&text, &Default::default())
        .ok()
        .flatten()?;
    let jsonc_parser::JsonValue::Object(object) = value else {
        return None;
    };
    let directory = path.parent().unwrap_or(Path::new("."));

    let mut raw = Raw::default();
    for extends in extends_values(&object) {
        if let Some(extended) = resolve_extends(directory, &extends)
            && let Some(parent) = load_file(&extended, depth + 1, stack)
        {
            raw.inherit(parent);
        }
    }
    if let Some(options) = object.get_object("compilerOptions") {
        raw.apply(options, directory);
    }

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
    base_url: Option<PathBuf>,
    paths: Option<BTreeMap<String, Vec<String>>>,
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
        ] {
            if let Some(value) = options.get_boolean(name) {
                *slot = Some(value);
            }
        }
        if let Some(value) = options.get_string("importsNotUsedAsValues") {
            self.imports_not_used_as_values = Some(value.as_ref().to_owned());
        }
        if let Some(value) = options.get_string("baseUrl") {
            self.base_url = Some(directory.join(value.as_ref()));
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
            self.paths = Some(map);
        }
    }

    fn into_config(self) -> Tsconfig {
        Tsconfig {
            missing: false,
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
            base_url: self.base_url,
            paths: self.paths.unwrap_or_default(),
        }
    }
}

fn merge_option<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

/// Internal import prefixes derived from `compilerOptions.paths` aliases.
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
}
