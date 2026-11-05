use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    pub experimental_decorators: bool,
    pub no_unused_locals: Option<bool>,
    pub no_unused_parameters: Option<bool>,
    pub base_url: Option<PathBuf>,
    pub paths: BTreeMap<String, Vec<String>>,
}

impl Tsconfig {
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

/// Loads the nearest config, following simple relative `extends` chains.
pub(crate) fn load_for(path: &Path) -> Tsconfig {
    nearest(path)
        .and_then(|path| load_file(&path, 0))
        .unwrap_or(Tsconfig {
            missing: true,
            ..Tsconfig::default()
        })
}

fn load_file(path: &Path, depth: usize) -> Option<Tsconfig> {
    if depth > 8 {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let value = jsonc_parser::parse_to_value(&text, &Default::default())
        .ok()
        .flatten()?;
    let jsonc_parser::JsonValue::Object(object) = value else {
        return None;
    };
    let directory = path.parent().unwrap_or(Path::new("."));

    let mut raw = Raw::default();
    if let Some(extends) = object.get_string("extends") {
        let extends = extends.as_ref();
        if is_relative(extends) {
            let extended = resolve_relative(directory, extends);
            if extended.is_file()
                && let Some(parent) = load_file(&extended, depth + 1)
            {
                raw = Raw::from(parent);
            }
        }
    }
    if let Some(options) = object.get_object("compilerOptions") {
        raw.apply(options, directory);
    }

    Some(raw.into_config())
}

fn is_relative(value: &str) -> bool {
    value.starts_with("./") || value.starts_with("../")
}

fn resolve_relative(directory: &Path, value: &str) -> PathBuf {
    let path = directory.join(value);
    if path.is_file() || path.extension().is_some() {
        path
    } else {
        path.with_extension("json")
    }
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

impl From<Tsconfig> for Raw {
    fn from(config: Tsconfig) -> Self {
        Self {
            jsx: config.jsx,
            jsx_factory: config.jsx_factory,
            jsx_fragment_factory: config.jsx_fragment_factory,
            verbatim_module_syntax: Some(config.verbatim_module_syntax),
            imports_not_used_as_values: config.imports_not_used_as_values,
            preserve_value_imports: Some(config.preserve_value_imports),
            emit_decorator_metadata: Some(config.emit_decorator_metadata),
            experimental_decorators: Some(config.experimental_decorators),
            no_unused_locals: config.no_unused_locals,
            no_unused_parameters: config.no_unused_parameters,
            base_url: config.base_url,
            paths: (!config.paths.is_empty()).then_some(config.paths),
        }
    }
}

impl Raw {
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
