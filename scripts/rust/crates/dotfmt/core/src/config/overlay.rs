use std::path::Path;
use std::sync::Arc;

use ignore::gitignore::GitignoreBuilder;
use serde_json::{Map, Value};

use crate::diagnostic::Diagnostic;
use crate::language::Language;

use super::patterns::add_pattern;
use super::{Layer, Local, PatternLayer, Setting, Settings};

#[derive(Clone, Debug)]
pub struct ConfigOverlay {
    pub(super) layer: Layer,
}

impl ConfigOverlay {
    pub fn from_json(value: &Value, root: &Path) -> Result<Self, Diagnostic> {
        let source = root.join(".vscode").join("settings.json");
        let mut layer = Layer {
            source: Some(source.clone()),
            ..Layer::default()
        };
        let Some(blocks) = value.as_object() else {
            return Err(error(&source, "configuration must be an object"));
        };
        let mut included = false;
        let mut excluded = false;
        for (raw_name, value) in blocks {
            match raw_name.replace('-', "_").as_str() {
                "global" => {
                    if value.is_null() {
                        continue;
                    }
                    let settings = value
                        .as_object()
                        .ok_or_else(|| error(&source, "global must be an object"))?;
                    for (key, value) in settings {
                        let key = setting_name(key);
                        if let Some(setting) = setting(&key, value, &source, true)?
                            && layer.global.insert(key.clone(), setting).is_some()
                        {
                            return Err(error(&source, format!("duplicate setting '{key}'")));
                        }
                    }
                }
                "included_files" | "include" => {
                    if value.is_null() {
                        continue;
                    }
                    if included {
                        return Err(error(&source, "duplicate included_files block"));
                    }
                    included = true;
                    layer.includes = Some(patterns(value, &source, root, "included_files")?);
                }
                "excluded_files" | "exclude" => {
                    if value.is_null() {
                        continue;
                    }
                    if excluded {
                        return Err(error(&source, "duplicate excluded_files block"));
                    }
                    excluded = true;
                    layer.excludes = Some(patterns(value, &source, root, "excluded_files")?);
                }
                _ => {
                    let language =
                        Language::parse(raw_name).map_err(|message| error(&source, message))?;
                    if value.is_null() {
                        continue;
                    }
                    if layer.languages.contains_key(&language) {
                        return Err(error(
                            &source,
                            format!("duplicate {} block", language.name()),
                        ));
                    }
                    let object = value.as_object().ok_or_else(|| {
                        error(&source, format!("{} must be an object", language.name()))
                    })?;
                    layer
                        .languages
                        .insert(language, language_block(object, &source, root, language)?);
                }
            }
        }
        Ok(Self { layer })
    }

    pub fn is_empty(&self) -> bool {
        self.layer.is_empty()
    }
}

fn language_block(
    object: &Map<String, Value>,
    source: &Path,
    root: &Path,
    language: Language,
) -> Result<Local, Diagnostic> {
    let mut local = Local {
        enabled: None,
        settings: Settings::new(),
        includes: None,
        excludes: None,
    };
    for (raw_name, value) in object {
        let name = setting_name(raw_name);
        match name.as_str() {
            "enabled" => {
                if value.is_null() {
                    continue;
                }
                local.enabled = Some(value.as_bool().ok_or_else(|| {
                    error(
                        source,
                        format!("{}.enabled must be true or false", language.name()),
                    )
                })?);
            }
            "include" | "included_files" => {
                if value.is_null() {
                    continue;
                }
                if local.includes.is_some() {
                    return Err(error(source, "duplicate included_files block"));
                }
                local.includes = Some(patterns(value, source, root, "included_files")?);
            }
            "exclude" | "excluded_files" => {
                if value.is_null() {
                    continue;
                }
                if local.excludes.is_some() {
                    return Err(error(source, "duplicate excluded_files block"));
                }
                local.excludes = Some(patterns(value, source, root, "excluded_files")?);
            }
            _ => {
                if let Some(setting) = setting(&name, value, source, false)?
                    && local.settings.insert(name.clone(), setting).is_some()
                {
                    return Err(error(source, format!("duplicate setting '{name}'")));
                }
            }
        }
    }
    Ok(local)
}

fn setting(
    key: &str,
    value: &Value,
    source: &Path,
    global: bool,
) -> Result<Option<Setting>, Diagnostic> {
    let Some(value) = scalar(key, value, source)? else {
        return Ok(None);
    };
    Ok(Some(Setting {
        value,
        source: source.to_path_buf(),
        line: 0,
        global,
    }))
}

fn scalar(key: &str, value: &Value, source: &Path) -> Result<Option<String>, Diagnostic> {
    Ok(match value {
        Value::Null => None,
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::String(value) => (!value.is_empty()).then(|| value.clone()),
        Value::Array(values) => {
            let mut parts = Vec::with_capacity(values.len());
            for value in values {
                match value {
                    Value::Null => continue,
                    Value::Bool(value) => parts.push(value.to_string()),
                    Value::Number(value) => parts.push(value.to_string()),
                    Value::String(value) => parts.push(value.clone()),
                    Value::Array(_) | Value::Object(_) => {
                        return Err(invalid_setting(key, source));
                    }
                }
            }
            if parts.iter().all(String::is_empty) {
                None
            } else {
                Some(parts.join(","))
            }
        }
        Value::Object(_) => return Err(invalid_setting(key, source)),
    })
}

fn invalid_setting(key: &str, source: &Path) -> Diagnostic {
    error(
        source,
        format!("setting '{key}' must be a string, number, boolean, or array of those"),
    )
}

fn patterns(
    value: &Value,
    source: &Path,
    root: &Path,
    block: &str,
) -> Result<Arc<PatternLayer>, Diagnostic> {
    let entries = value
        .as_array()
        .ok_or_else(|| error(source, format!("{block} must be an array of strings")))?;
    let mut builder = GitignoreBuilder::new(root);
    for entry in entries {
        let pattern = entry
            .as_str()
            .ok_or_else(|| error(source, format!("{block} entries must be strings")))?;
        add_pattern(&mut builder, source, pattern).map_err(|pattern_error| {
            error(
                source,
                format!("invalid {block} pattern '{pattern}': {pattern_error}"),
            )
        })?;
    }
    let matcher = builder.build().map_err(|pattern_error| {
        error(source, format!("invalid {block} pattern: {pattern_error}"))
    })?;
    Ok(Arc::new(PatternLayer {
        root: root.to_path_buf(),
        matcher,
    }))
}

fn setting_name(name: &str) -> String {
    name.replace('-', "_")
}

fn error(source: &Path, message: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::config(source, 0, message.to_string())
}
