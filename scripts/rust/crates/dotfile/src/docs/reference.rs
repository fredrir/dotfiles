use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Deserialize;

use super::markdown::{block, replace_block, table};
use super::plan::{Output, read};
use crate::context::Context;
use crate::surface::metadata::{self, Command, Param};

#[derive(Deserialize)]
struct Page {
    name: String,
    title: String,
    programs: Vec<String>,
    source: String,
}
#[derive(Deserialize)]
struct Catalog {
    pages: Vec<Page>,
    native: Vec<String>,
    standard: BTreeMap<String, String>,
    commands: BTreeMap<String, String>,
    flags: BTreeMap<String, BTreeMap<String, String>>,
}
const CATALOG: &str = include_str!("../../assets/cli-reference.json");
const STANDARD: &[&str] = &["--help", "--completions", "--version"];

pub(super) fn outputs(context: &Context) -> Result<(Vec<Output>, Vec<String>), String> {
    let catalog: Catalog = serde_json::from_str(CATALOG).map_err(|e| e.to_string())?;
    let mut trees = metadata::declared(context)?.commands;
    trees.extend(metadata::external_many(
        context,
        &catalog
            .native
            .iter()
            .filter(|name| name.as_str() != "dotfile")
            .cloned()
            .collect::<Vec<_>>(),
    )?);
    trees.insert("dotfile".into(), metadata::native());
    let mut outputs = Vec::new();
    let mut missing = BTreeSet::new();
    for page in &catalog.pages {
        let absent = page
            .programs
            .iter()
            .filter(|p| !trees.contains_key(*p))
            .cloned()
            .collect::<Vec<_>>();
        if !absent.is_empty() {
            if context.root.join(&page.source).exists() {
                missing.extend(absent);
            }
            continue;
        }
        let roots = page
            .programs
            .iter()
            .filter_map(|p| trees.get(p))
            .collect::<Vec<_>>();
        let relative = PathBuf::from(format!("docs/cli/{}.md", page.name));
        let path = context.root.join(&relative);
        let previous = read(&path)?;
        let updated = page_text(page, &roots, &previous, &catalog)
            .map_err(|error| format!("{}: {error}", relative.display()))?;
        outputs.push(Output::text(relative, updated));
    }
    let relative = PathBuf::from("docs/cli/_INDEX.md");
    let path = context.root.join(&relative);
    let previous = read(&path)?;
    let rows = catalog
        .pages
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                format!("[{}.md](./{}.md)", p.name, p.name),
                format!("[{}]", p.source),
            ]
        })
        .collect::<Vec<_>>();
    let body = table(&["Command", "Docs", "Path"], &rows);
    let updated = if previous.is_empty() {
        format!(
            "# Command Line Interface (CLI)\n\n{}\n",
            block("cli:index", &body)
        )
    } else {
        replace_block(&previous, "cli:index", &body)
            .map_err(|error| format!("{}: {error}", relative.display()))?
    };
    outputs.push(Output::text(relative, updated));
    Ok((outputs, missing.into_iter().collect()))
}

fn page_text(
    page: &Page,
    roots: &[&Command],
    previous: &str,
    catalog: &Catalog,
) -> Result<String, String> {
    let commands = roots
        .iter()
        .flat_map(|root| root.walk())
        .collect::<Vec<_>>();
    let rows = commands
        .iter()
        .map(|c| {
            let label = c.label();
            vec![
                format!("`{label}`"),
                catalog
                    .commands
                    .get(&label)
                    .cloned()
                    .unwrap_or_else(|| c.help.clone()),
            ]
        })
        .collect::<Vec<_>>();
    let command_table = table(&["Command", "Description"], &rows);
    let mut seen = BTreeSet::new();
    let mut own = Vec::new();
    let mut standard = BTreeMap::new();
    for c in commands {
        for p in &c.params {
            if p.kind != "option" || p.hidden || !seen.insert(p.flag().to_string()) {
                continue;
            }
            if p.standard() {
                standard.insert(p.flag().to_string(), p.clone());
            } else {
                own.push(p.clone());
            }
        }
    }
    standard.entry("--help".into()).or_insert_with(|| Param {
        opts: vec!["--help".into()],
        ..Default::default()
    });
    for flag in STANDARD {
        if let Some(p) = standard.remove(*flag) {
            own.push(p);
        }
    }
    let rows = own
        .iter()
        .map(|p| {
            let description = if p.standard() {
                catalog.standard.get(p.flag())
            } else {
                catalog.flags.get(&page.name).and_then(|f| f.get(p.flag()))
            };
            vec![
                p.spelling(),
                description.cloned().unwrap_or_else(|| p.help.clone()),
            ]
        })
        .collect::<Vec<_>>();
    let flags = table(&["Flag", "Description"], &rows);
    if !previous.is_empty() {
        let updated = replace_block(previous, "cli:commands", &command_table)?;
        return replace_block(&updated, "cli:flags", &flags);
    }
    let mut body = format!(
        "# {}\n\n## Commands\n\n{}\n",
        page.title,
        block("cli:commands", &command_table)
    );
    if !rows.is_empty() {
        body.push_str(&format!("\n## Flags\n\n{}\n", block("cli:flags", &flags)));
    }
    Ok(body)
}
