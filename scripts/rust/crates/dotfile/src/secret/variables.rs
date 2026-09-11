use crate::context::Context;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub struct Variables {
    pub values: BTreeMap<String, String>,
    pub ok: bool,
    pub note: String,
}

pub fn load_variables(context: &Context) -> Variables {
    let source = context.root.join("vars.enc.yaml");
    if !source.is_file() {
        return Variables {
            values: BTreeMap::new(),
            ok: true,
            note: String::new(),
        };
    }
    if !super::vault::identity_path(context).is_file() {
        return Variables {
            values: BTreeMap::new(),
            ok: false,
            note: "vars.enc.yaml needs an age identity to read".to_string(),
        };
    }
    let output = match super::sops::decrypt(context, &source, None, true) {
        Ok(output) => output,
        Err(note) => {
            return Variables {
                values: BTreeMap::new(),
                ok: false,
                note,
            };
        }
    };
    let Ok(Value::Object(document)) = serde_json::from_slice::<Value>(&output) else {
        return Variables {
            values: BTreeMap::new(),
            ok: false,
            note: "vars.enc.yaml must hold a mapping".to_string(),
        };
    };
    let mut values = BTreeMap::new();
    match flatten_variables(&document, "", &mut values) {
        Ok(()) => Variables {
            values,
            ok: true,
            note: String::new(),
        },
        Err(note) => Variables {
            values: BTreeMap::new(),
            ok: false,
            note,
        },
    }
}

pub fn flatten_variables(
    values: &serde_json::Map<String, Value>,
    prefix: &str,
    output: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in values {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(children) => flatten_variables(children, &name, output)?,
            Value::Array(_) => {
                return Err(format!("'{name}' is a list; a var must be a single value"));
            }
            Value::Null => return Err(format!("'{name}' has no value")),
            Value::Bool(value) => {
                output.insert(name, value.to_string());
            }
            Value::String(value) => {
                output.insert(name, value.clone());
            }
            Value::Number(value) => {
                output.insert(name, value.to_string());
            }
        }
    }
    Ok(())
}

static PLACEHOLDER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*([A-Za-z0-9_][A-Za-z0-9_.-]*)\s*\}\}")
        .expect("valid placeholder pattern")
});

pub fn references(template: &str) -> Vec<String> {
    PLACEHOLDER
        .captures_iter(template)
        .map(|capture| capture[1].to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn render_template(template: &str, values: &BTreeMap<String, String>) -> (String, Vec<String>) {
    let mut missing = BTreeSet::new();
    let output = PLACEHOLDER.replace_all(template, |capture: &regex::Captures<'_>| {
        if let Some(value) = values.get(&capture[1]) {
            value.clone()
        } else {
            missing.insert(capture[1].to_string());
            capture[0].to_string()
        }
    });
    (output.into_owned(), missing.into_iter().collect())
}

impl Drop for Variables {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.values.values_mut() {
            value.zeroize();
        }
    }
}
