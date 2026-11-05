use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Map, Value};

use crate::args::Cli;

pub fn run(cli: &Cli, source: &str) -> Result<ExitCode, String> {
    if source != "prettier" {
        return Err(format!("unsupported migration source: {source}"));
    }
    let root = cli
        .targets
        .first()
        .map_or_else(|| PathBuf::from("."), PathBuf::clone);
    let root = if root.is_file() {
        root.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        root
    };
    let mut config = Map::new();
    let mut warnings = Vec::new();
    read_package_json(&root, &mut config, &mut warnings)?;
    read_rc(&root, &mut config, &mut warnings)?;
    let output = render(&config, &mut warnings);
    print!("{output}");
    for warning in warnings {
        eprintln!("dotfmt: migrate: {warning}");
    }
    Ok(ExitCode::SUCCESS)
}

fn read_package_json(
    root: &Path,
    config: &mut Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let path = root.join("package.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let package: Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if let Some(prettier) = package.get("prettier") {
        merge_prettier(prettier, config, warnings);
    }
    Ok(())
}

fn read_rc(
    root: &Path,
    config: &mut Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    for name in [".prettierrc", ".prettierrc.json"] {
        let path = root.join(name);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => merge_prettier(&value, config, warnings),
            Err(error) => warnings.push(format!("{}: ignored: {error}", path.display())),
        }
    }
    for name in [
        ".prettierrc.js",
        ".prettierrc.cjs",
        ".prettierrc.mjs",
        ".prettierrc.ts",
        "prettier.config.js",
        "prettier.config.cjs",
        "prettier.config.mjs",
        "prettier.config.ts",
    ] {
        if root.join(name).is_file() {
            warnings.push(format!(
                "{name} is JavaScript; export a JSON config or copy its values manually"
            ));
        }
    }
    Ok(())
}

fn merge_prettier(value: &Value, config: &mut Map<String, Value>, warnings: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        warnings.push("prettier config is not an object".to_owned());
        return;
    };
    for (key, value) in object {
        if key == "overrides" {
            warnings.push(
                "Prettier overrides are not migrated; use language blocks or included_files"
                    .to_owned(),
            );
            continue;
        }
        config.insert(key.clone(), value.clone());
    }
}

fn render(config: &Map<String, Value>, warnings: &mut Vec<String>) -> String {
    let width = number(config, "printWidth").unwrap_or(80);
    let indent = number(config, "tabWidth").unwrap_or(2);
    let tabs = boolean(config, "useTabs").unwrap_or(false);
    let single = boolean(config, "singleQuote").unwrap_or(false);
    let jsx_single = boolean(config, "jsxSingleQuote").unwrap_or(single);
    let semi = boolean(config, "semi").unwrap_or(true);
    let trailing = string(config, "trailingComma").unwrap_or("all");
    let arrow = string(config, "arrowParens").unwrap_or("always");
    let quote_props = string(config, "quoteProps").unwrap_or("as-needed");
    let bracket_spacing = boolean(config, "bracketSpacing").unwrap_or(true);
    let bracket_same_line = boolean(config, "bracketSameLine").unwrap_or(false);
    let line_endings = match string(config, "endOfLine").unwrap_or("lf") {
        "crlf" => "windows",
        _ => "unix",
    };
    let plugins = config
        .get("plugins")
        .and_then(Value::as_array)
        .map(|plugins| plugins.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let organize = plugins
        .iter()
        .any(|plugin| plugin.contains("organize-imports") || plugin.contains("sort-imports"));
    let tailwind = plugins.iter().any(|plugin| plugin.contains("tailwind"));
    for plugin in &plugins {
        if !plugin.contains("organize-imports")
            && !plugin.contains("sort-imports")
            && !plugin.contains("tailwind")
        {
            warnings.push(format!("unsupported Prettier plugin: {plugin}"));
        }
    }
    if let Some(functions) = config.get("tailwindFunctions").and_then(Value::as_array) {
        let _ = functions;
    }
    let trailing = match trailing {
        "none" => "none",
        "es5" => "es5",
        _ => "all",
    };
    let tailwind_functions = string_list(config, "tailwindFunctions")
        .unwrap_or_else(|| "cn, clsx, cva, twMerge".to_owned());
    let tailwind_attributes =
        string_list(config, "tailwindAttributes").unwrap_or_else(|| "class, className".to_owned());

    let mut output = String::new();
    output.push_str("# Migrated from Prettier. Review plugin-specific options.\n\n");
    output.push_str("typescript {\n");
    output.push_str("  enabled               = true\n");
    output.push_str(&format!("  width                 = {width}\n"));
    output.push_str(&format!("  indent                = {indent}\n"));
    output.push_str(&format!(
        "  indent_type           = {}\n",
        if tabs { "tabs" } else { "spaces" }
    ));
    output.push_str(&format!("  line_endings          = {line_endings}\n"));
    output.push_str("  final_newline         = true\n");
    output.push_str(&format!(
        "  quote_style           = {}\n",
        if single { "single" } else { "double" }
    ));
    output.push_str(&format!(
        "  jsx_quote_style       = {}\n",
        if jsx_single { "single" } else { "double" }
    ));
    output.push_str(&format!("  quote_properties      = {quote_props}\n"));
    output.push_str(&format!(
        "  semicolons            = {}\n",
        if semi { "always" } else { "as-needed" }
    ));
    output.push_str(&format!("  trailing_commas       = {trailing}\n"));
    output.push_str(&format!(
        "  arrow_parentheses     = {}\n",
        if arrow == "avoid" {
            "as-needed"
        } else {
            "always"
        }
    ));
    output.push_str(&format!("  bracket_spacing       = {bracket_spacing}\n"));
    output.push_str(&format!("  bracket_same_line     = {bracket_same_line}\n"));
    output.push_str(&format!("  sort_imports          = {organize}\n"));
    output.push_str(&format!(
        "  tailwind              = {}\n",
        if tailwind { "auto" } else { "off" }
    ));
    output.push_str(&format!(
        "  tailwind_functions    = \"{tailwind_functions}\"\n"
    ));
    output.push_str(&format!(
        "  tailwind_attributes   = \"{tailwind_attributes}\"\n"
    ));
    output.push_str("}\n\n");
    output.push_str("css {\n  enabled = true\n");
    output.push_str(&format!("  width = {width}\n  indent = {indent}\n"));
    output.push_str(&format!(
        "  indent_type = {}\n",
        if tabs { "tabs" } else { "spaces" }
    ));
    output.push_str(&format!("  line_endings = {line_endings}\n"));
    output.push_str("  final_newline = true\n");
    output.push_str(&format!(
        "  quotes = {}\n",
        if single { "single" } else { "double" }
    ));
    output.push_str("}\n\n");
    output.push_str("html {\n  enabled = true\n");
    output.push_str(&format!("  width = {width}\n  indent = {indent}\n"));
    output.push_str(&format!(
        "  indent_type = {}\n",
        if tabs { "tabs" } else { "spaces" }
    ));
    output.push_str(&format!("  line_endings = {line_endings}\n"));
    output.push_str("  final_newline = true\n");
    output.push_str(&format!(
        "  quotes = {}\n",
        if single { "single" } else { "double" }
    ));
    output.push_str("}\n");
    output
}

fn number(config: &Map<String, Value>, key: &str) -> Option<usize> {
    config
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn boolean(config: &Map<String, Value>, key: &str) -> Option<bool> {
    config.get(key).and_then(Value::as_bool)
}

fn string<'a>(config: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    config.get(key).and_then(Value::as_str)
}

fn string_list(config: &Map<String, Value>, key: &str) -> Option<String> {
    config.get(key).and_then(Value::as_array).map(|values| {
        values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    })
}
