use std::fs;
use std::path::PathBuf;

use crate::context::Context;

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

#[derive(Debug, Default)]
pub struct Npmrc {
    // Highest precedence first: environment, project, user, global.
    layers: Vec<Vec<(String, String)>>,
}

impl Npmrc {
    pub fn load(ctx: &Context) -> Npmrc {
        let mut layers = vec![environment(ctx)];
        let project = ctx
            .ancestors()
            .find(|dir| dir.join("package.json").is_file())
            .map(|dir| dir.join(".npmrc"));
        let user = ctx
            .var_path("NPM_CONFIG_USERCONFIG")
            .or_else(|| ctx.var_path("npm_config_userconfig"))
            .unwrap_or_else(|| ctx.home.join(".npmrc"));
        let files: Vec<PathBuf> = project.into_iter().chain([user]).collect();
        for file in files {
            if let Ok(text) = fs::read_to_string(&file) {
                layers.push(parse(&text, ctx));
            }
        }
        Npmrc { layers }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.layers.iter().find_map(|layer| {
            layer
                .iter()
                .find(|(known, _)| known.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.clone())
        })
    }

    pub fn registry_for(&self, package: &str) -> String {
        let scoped = package
            .strip_prefix('@')
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(scope, _)| self.get(&format!("@{scope}:registry")));
        let registry = scoped
            .or_else(|| self.get("registry"))
            .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
        registry.trim_end_matches('/').to_string()
    }
}

fn environment(ctx: &Context) -> Vec<(String, String)> {
    ["registry", "prefix", "cache"]
        .iter()
        .filter_map(|key| {
            ctx.var(&format!("npm_config_{key}"))
                .or_else(|| ctx.var(&format!("NPM_CONFIG_{}", key.to_ascii_uppercase())))
                .map(|value| (key.to_string(), value.to_string()))
        })
        .collect()
}

fn parse(text: &str, ctx: &Context) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(['#', ';']))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let value = value.trim().trim_matches('"');
            (key.trim().to_string(), interpolate(value, ctx))
        })
        .collect()
}

fn interpolate(value: &str, ctx: &Context) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        out.push_str(ctx.var(&rest[start + 2..start + end]).unwrap_or(""));
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "../../tests/unit/node/npmrc_tests.rs"]
mod tests;
