use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::{DirEntry, WalkBuilder};

use crate::tsconfig::{self, Tsconfig};

pub(crate) const SOURCE_EXTENSIONS: &[&str] =
    &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
const DECLARATION_EXTENSIONS: &[&str] = &["d.ts", "d.mts", "d.cts"];
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
const MAX_GLOB_RESULTS: usize = 10_000;
const MAX_GLOB_EXPANSIONS: usize = 64;

pub(crate) struct Resolver {
    root: PathBuf,
    configs: HashMap<PathBuf, Arc<Vec<Tsconfig>>>,
}

impl Resolver {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            configs: HashMap::new(),
        }
    }

    pub(crate) fn resolve(&mut self, importer: &Path, specifier: &str) -> Option<PathBuf> {
        let specifier = specifier.trim();
        if specifier.is_empty() {
            return None;
        }
        if Path::new(specifier).is_absolute() {
            return resolve_target(Path::new(specifier));
        }
        if specifier.starts_with('.') {
            return resolve_target(&importer.parent()?.join(specifier));
        }
        let configs = self.configs_for(importer);
        if specifier.starts_with('#') {
            for config in configs.iter() {
                if let Some(path) = resolve_paths(config, specifier) {
                    return Some(path);
                }
            }
            return resolve_package_imports(importer, specifier);
        }

        for config in configs.iter() {
            if let Some(path) = resolve_paths(config, specifier) {
                return Some(path);
            }
            if let Some(base_url) = &config.base_url
                && let Some(path) = resolve_target(&base_url.join(specifier))
            {
                return Some(path);
            }
        }
        resolve_bare(importer, specifier)
    }

    pub(crate) fn configs_for(&mut self, importer: &Path) -> Arc<Vec<Tsconfig>> {
        let directory = importer
            .parent()
            .map_or_else(|| self.root.clone(), Path::to_path_buf);
        Arc::clone(
            self.configs
                .entry(directory)
                .or_insert_with_key(|directory| {
                    Arc::new(tsconfig::resolution_configs_for(
                        &directory.join("module.ts"),
                        &self.root,
                    ))
                }),
        )
    }
}

pub(crate) fn resolve_target(path: &Path) -> Option<PathBuf> {
    let path = normalize(path);
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return resolve_extensionless(&path);
    };
    let extension = extension.to_ascii_lowercase();
    if let Some(replacements) = replaceable_extensions(&extension) {
        let without = path.with_extension("");
        for replacement in replacements {
            let candidate = without.with_extension(replacement);
            if candidate.is_file() {
                return canonicalize(candidate);
            }
        }
        return path.is_file().then(|| canonicalize(path)).flatten();
    }
    if path.is_file() {
        return canonicalize(path);
    }
    if SOURCE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    resolve_extensionless(&path)
}

fn resolve_extensionless(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return canonicalize(path.to_path_buf());
    }
    for extension in SOURCE_EXTENSIONS {
        let candidate = append_extension(path, extension);
        if candidate.is_file() {
            return canonicalize(candidate);
        }
    }
    for extension in DECLARATION_EXTENSIONS {
        let candidate = append_extension(path, extension);
        if candidate.is_file() {
            return canonicalize(candidate);
        }
    }
    for extension in SOURCE_EXTENSIONS {
        let candidate = path.join(format!("index.{extension}"));
        if candidate.is_file() {
            return canonicalize(candidate);
        }
    }
    for extension in DECLARATION_EXTENSIONS {
        let candidate = path.join(format!("index.{extension}"));
        if candidate.is_file() {
            return canonicalize(candidate);
        }
    }
    resolve_directory_package(path)
}

fn resolve_directory_package(path: &Path) -> Option<PathBuf> {
    if !path.is_dir() {
        return None;
    }
    let text = fs::read_to_string(path.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    package_targets(path, &value)
        .into_iter()
        .find_map(|candidate| resolve_target(&candidate))
}

fn replaceable_extensions(extension: &str) -> Option<&'static [&'static str]> {
    match extension {
        "js" => Some(&["ts", "tsx", "js"]),
        "jsx" => Some(&["tsx", "jsx"]),
        "mjs" => Some(&["mts", "d.mts", "mjs"]),
        "cjs" => Some(&["cts", "d.cts", "cjs"]),
        _ => None,
    }
}

fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(".");
    value.push(extension);
    PathBuf::from(value)
}

fn canonicalize(path: PathBuf) -> Option<PathBuf> {
    fs::canonicalize(&path).ok().or(Some(path))
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

fn resolve_paths(config: &Tsconfig, specifier: &str) -> Option<PathBuf> {
    if config.paths.is_empty() {
        return None;
    }
    let mut best: Option<(&[String], Option<&str>, usize)> = None;
    for (pattern, targets) in &config.paths {
        let Some((prefix, suffix, capture)) = match_alias(pattern, specifier) else {
            continue;
        };
        let length = prefix.len() + suffix.map_or(0, str::len);
        if best.is_none_or(|(_, _, best_length)| length > best_length) {
            best = Some((targets, capture, length));
        }
    }
    let (targets, capture, _) = best?;
    for target in targets {
        let substituted = match capture {
            Some(capture) => target.replace('*', capture),
            None => target.clone(),
        };
        if let Some(path) = resolve_path_target(config, &substituted) {
            return Some(path);
        }
    }
    None
}

fn resolve_path_target(config: &Tsconfig, target: &str) -> Option<PathBuf> {
    let path = Path::new(target);
    if path.is_absolute() {
        return resolve_target(path);
    }
    let base = config
        .base_url
        .as_deref()
        .or(config.paths_directory.as_deref())
        .unwrap_or(&config.directory);
    resolve_target(&base.join(path))
}

fn match_alias<'p, 's>(
    pattern: &'p str,
    specifier: &'s str,
) -> Option<(&'p str, Option<&'p str>, Option<&'s str>)> {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            if specifier.len() < prefix.len() + suffix.len()
                || !specifier.starts_with(prefix)
                || !specifier.ends_with(suffix)
            {
                return None;
            }
            let end = specifier.len() - suffix.len();
            Some((prefix, Some(suffix), Some(&specifier[prefix.len()..end])))
        }
        None => (pattern == specifier).then_some((pattern, None, None)),
    }
}

fn resolve_bare(importer: &Path, specifier: &str) -> Option<PathBuf> {
    let (name, subpath) = split_package(specifier)?;
    let mut directory = importer.parent()?;
    loop {
        let package = directory.join("node_modules").join(&name);
        if package.is_dir()
            && let Some(path) = resolve_package_target(&package, subpath)
        {
            return Some(path);
        }
        directory = directory.parent()?;
    }
}

fn resolve_package_target(package: &Path, subpath: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(package.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if subpath.is_empty() {
        for candidate in package_targets(package, &value) {
            if let Some(path) = resolve_target(&candidate) {
                return Some(path);
            }
        }
        return None;
    }
    let mut candidates = Vec::new();
    collect_subpath_exports(value.get("exports"), subpath, &mut candidates);
    candidates.push(subpath.to_owned());
    for candidate in candidates {
        let relative = candidate.trim_start_matches("./");
        if let Some(source) = source_directory_swap(package, relative)
            && let Some(path) = resolve_target(&source)
        {
            return Some(path);
        }
        if let Some(path) = resolve_target(&package.join(relative)) {
            return Some(path);
        }
    }
    None
}

fn package_targets(package: &Path, value: &serde_json::Value) -> Vec<PathBuf> {
    let mut values = Vec::new();
    for key in ["types", "typings", "module", "main"] {
        if let Some(value) = value.get(key).and_then(serde_json::Value::as_str) {
            values.push(value.to_owned());
        }
    }
    collect_export_strings(value.get("exports"), &mut values);
    let mut candidates = Vec::new();
    for value in values {
        let relative = value.strip_prefix("./").unwrap_or(&value);
        if relative.contains('*') || relative.is_empty() {
            continue;
        }
        let direct = package.join(relative);
        if let Some(source) = source_directory_swap(package, relative) {
            candidates.push(source);
        }
        candidates.push(direct);
    }
    candidates
}

pub(crate) fn source_directory_swap(package: &Path, relative: &str) -> Option<PathBuf> {
    let (first, rest) = relative.split_once('/')?;
    if !matches!(
        first,
        "dist" | "out" | "build" | "lib" | "esm" | "cjs" | "umd"
    ) {
        return None;
    }
    Some(package.join("src").join(rest))
}

fn collect_subpath_exports(
    value: Option<&serde_json::Value>,
    subpath: &str,
    candidates: &mut Vec<String>,
) {
    let Some(serde_json::Value::Object(map)) = value else {
        return;
    };
    let wanted = format!("./{subpath}");
    let mut best: Option<(&str, &str)> = None;
    for (key, value) in map {
        if key == &wanted {
            collect_export_strings(Some(value), candidates);
            continue;
        }
        if let Some((prefix, suffix)) = key.split_once('*')
            && wanted.len() >= prefix.len() + suffix.len()
            && wanted.starts_with(prefix)
            && wanted.ends_with(suffix)
        {
            let length = prefix.len() + suffix.len();
            if best.is_none_or(|(best_prefix, best_suffix)| {
                length > best_prefix.len() + best_suffix.len()
            }) {
                best = Some((prefix, suffix));
            }
        }
    }
    if let Some((prefix, suffix)) = best {
        let capture = &wanted[prefix.len()..wanted.len() - suffix.len()];
        let mut targets = Vec::new();
        for (key, value) in map {
            if key.contains('*') && key.starts_with(prefix) && key.ends_with(suffix) {
                collect_export_strings(Some(value), &mut targets);
            }
        }
        for target in targets {
            candidates.push(target.replace('*', capture));
        }
    }
}

pub(crate) fn collect_export_strings(value: Option<&serde_json::Value>, values: &mut Vec<String>) {
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

fn resolve_package_imports(importer: &Path, specifier: &str) -> Option<PathBuf> {
    let mut directory = importer.parent()?;
    loop {
        let package_json = directory.join("package.json");
        if let Ok(text) = fs::read_to_string(&package_json) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                return None;
            };
            let imports = value
                .get("imports")
                .and_then(serde_json::Value::as_object)?;
            let mut candidates = Vec::new();
            if let Some(target) = imports.get(specifier) {
                collect_export_strings(Some(target), &mut candidates);
            }
            let mut best: Option<(&str, &str)> = None;
            for key in imports.keys() {
                if let Some((prefix, suffix)) = key.split_once('*')
                    && specifier.len() >= prefix.len() + suffix.len()
                    && specifier.starts_with(prefix)
                    && specifier.ends_with(suffix)
                    && best.is_none_or(|(best_prefix, best_suffix)| {
                        prefix.len() + suffix.len() > best_prefix.len() + best_suffix.len()
                    })
                {
                    best = Some((prefix, suffix));
                }
            }
            if let Some((prefix, suffix)) = best {
                let capture = &specifier[prefix.len()..specifier.len() - suffix.len()];
                let mut targets = Vec::new();
                for (key, value) in imports {
                    if key.contains('*') && key.starts_with(prefix) && key.ends_with(suffix) {
                        collect_export_strings(Some(value), &mut targets);
                    }
                }
                candidates.extend(
                    targets
                        .into_iter()
                        .map(|target| target.replace('*', capture)),
                );
            }
            for candidate in candidates {
                let resolved = if candidate.starts_with('.') {
                    resolve_target(&directory.join(candidate.trim_start_matches("./")))
                } else {
                    resolve_bare(importer, &candidate)
                };
                if let Some(path) = resolved {
                    return Some(path);
                }
            }
            return None;
        }
        directory = directory.parent()?;
    }
}

pub(crate) fn expand_glob(root: &Path, importer: &Path, pattern: &str) -> Vec<PathBuf> {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.starts_with('!') {
        return Vec::new();
    }
    let Some(patterns) = brace_expand(pattern) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for pattern in patterns {
        files.extend(expand_glob_pattern(root, importer, &pattern));
        if files.len() >= MAX_GLOB_RESULTS {
            break;
        }
    }
    files.sort();
    files.dedup();
    files.truncate(MAX_GLOB_RESULTS);
    files
}

pub(crate) fn glob_pattern_supported(pattern: &str) -> bool {
    let pattern = pattern.trim();
    !pattern.is_empty() && !pattern.starts_with('!') && brace_expand(pattern).is_some()
}

fn expand_glob_pattern(root: &Path, importer: &Path, pattern: &str) -> Vec<PathBuf> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Vec::new();
    }
    let (base, remaining) = if let Some(rest) = pattern.strip_prefix('/') {
        (root.to_path_buf(), rest)
    } else {
        let Some(parent) = importer.parent() else {
            return Vec::new();
        };
        (parent.to_path_buf(), pattern.trim_start_matches("./"))
    };
    let mut pattern_components: Vec<&str> = remaining
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let mut base = normalize(&base);
    while let Some(component) = pattern_components.first()
        && !contains_glob(component)
    {
        base.push(component);
        pattern_components.remove(0);
    }
    if pattern_components.is_empty() {
        return resolve_target(&base).into_iter().collect();
    }
    if !base.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    let mut builder = WalkBuilder::new(&base);
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
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(&base) else {
            continue;
        };
        let parts: Vec<&str> = relative
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect();
        if matches_components(&pattern_components, &parts) {
            files.push(path.to_path_buf());
            if files.len() >= MAX_GLOB_RESULTS {
                break;
            }
        }
    }
    files.sort();
    files
}

fn contains_glob(component: &str) -> bool {
    component.contains('*') || component.contains('?')
}

fn brace_expand(pattern: &str) -> Option<Vec<String>> {
    if pattern.contains(['[', ']', '(', ')', '\\']) {
        return None;
    }
    let Some(open) = pattern.find('{') else {
        return Some(vec![pattern.to_owned()]);
    };
    let mut depth = 0usize;
    let mut close = None;
    for (index, ch) in pattern.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let alternatives = split_alternatives(&pattern[open + 1..close]);
    if alternatives.len() < 2 {
        return None;
    }
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let mut expanded = Vec::new();
    for alternative in alternatives {
        for pattern in brace_expand(&format!("{prefix}{alternative}{suffix}"))? {
            expanded.push(pattern);
            if expanded.len() > MAX_GLOB_EXPANSIONS {
                return None;
            }
        }
    }
    Some(expanded)
}

fn split_alternatives(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
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

fn matches_components(pattern: &[&str], parts: &[&str]) -> bool {
    let Some((first, rest)) = pattern.split_first() else {
        return parts.is_empty();
    };
    if *first == "**" {
        return matches_components(rest, parts)
            || (!parts.is_empty() && matches_components(pattern, &parts[1..]));
    }
    let Some((part, remaining)) = parts.split_first() else {
        return false;
    };
    wildcard_match(first, part) && matches_components(rest, remaining)
}

pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pattern_index, mut text_index) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some((pattern_index, text_index));
            pattern_index += 1;
        } else if let Some((star_index, star_text)) = star {
            pattern_index = star_index + 1;
            text_index = star_text + 1;
            star = Some((star_index, star_text + 1));
        } else {
            return false;
        }
    }
    pattern[pattern_index..].iter().all(|byte| *byte == b'*')
}
