use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};
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

pub fn synchronize(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
) -> Result<usize, String> {
    let stamp = context.state.join("sync/docs.fingerprint");
    let before = fingerprint(context)?;
    if fs::read_to_string(&stamp).ok().as_deref() == Some(before.as_str()) {
        return Ok(0);
    }
    events.emit(Event::PhaseStarted {
        phase: Phase::Artifacts,
        total: None,
    });
    let (mut paths, missing) = generate(context, dry_run)?;
    for program in missing {
        events.emit(Event::Warning {
            message: format!("{program}: command metadata unavailable; documentation retained"),
            hint: None,
        });
    }
    paths.extend(doc_keybinds::generate(&context.root, dry_run)?);
    for path in &paths {
        events.emit(Event::Item {
            action: Action::Generate,
            path: path.clone(),
            detail: if dry_run { "would update" } else { "updated" }.into(),
            changed: true,
        });
    }
    if !dry_run {
        crate::context::write_atomic(&stamp, fingerprint(context)?.as_bytes())?;
    }
    Ok(paths.len())
}

pub fn generate(context: &Context, check: bool) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    let catalog: Catalog = serde_json::from_str(CATALOG).map_err(|e| e.to_string())?;
    let python = metadata::python(context)?;
    if !python.commands.is_empty()
        && python.source_fingerprint != metadata::python_fingerprint(context)?
    {
        return Err("Python command metadata is stale; run ./setup.sh --commands-only".into());
    }
    if python.commands.is_empty() && context.root.join("scripts/python/src/tools").is_dir() {
        return Err("Python command metadata missing; run ./setup.sh --commands-only".into());
    }
    let mut trees = python.commands;
    trees.insert("dotfile".into(), metadata::native());
    let mut changed = Vec::new();
    let mut missing = BTreeSet::new();
    for page in &catalog.pages {
        for program in &page.programs {
            if !trees.contains_key(program)
                && catalog.native.contains(program)
                && let Some(tree) = metadata::external(context, program)?
            {
                trees.insert(program.clone(), tree);
            }
        }
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
        let updated = page_text(page, &roots, &previous, &catalog);
        if updated != previous {
            if !check {
                crate::context::write_atomic(&path, updated.as_bytes())?;
            }
            changed.push(relative);
        }
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
    let updated = replace_block(&previous, "cli:index", &body).unwrap_or_else(|| {
        format!(
            "# Command Line Interface (CLI)\n\n{}\n",
            block("cli:index", &body)
        )
    });
    if updated != previous {
        if !check {
            crate::context::write_atomic(&path, updated.as_bytes())?;
        }
        changed.push(relative);
    }
    Ok((changed, missing.into_iter().collect()))
}

fn page_text(page: &Page, roots: &[&Command], previous: &str, catalog: &Catalog) -> String {
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
    if let Some(updated) = replace_block(previous, "cli:commands", &command_table)
        && let Some(updated) = replace_block(&updated, "cli:flags", &flags)
    {
        return updated;
    }
    let mut body = format!(
        "# {}\n\n## Commands\n\n{}\n",
        page.title,
        block("cli:commands", &command_table)
    );
    if !rows.is_empty() {
        body.push_str(&format!("\n## Flags\n\n{}\n", block("cli:flags", &flags)));
    }
    body
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let rows = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.replace('|', "\\|"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let widths = (0..headers.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain(std::iter::once(headers[i].chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let line = |row: &[String]| {
        format!(
            "| {} |",
            row.iter()
                .zip(&widths)
                .map(|(cell, width)| format!(
                    "{cell}{}",
                    " ".repeat(width.saturating_sub(cell.chars().count()))
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    let mut lines = vec![
        line(&headers.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        line(&widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>()),
    ];
    lines.extend(rows.iter().map(|row| line(row)));
    lines.join("\n")
}
fn block(name: &str, body: &str) -> String {
    format!("<!-- {name}:start -->\n{body}\n<!-- {name}:end -->")
}
fn replace_block(text: &str, name: &str, body: &str) -> Option<String> {
    let start = format!("<!-- {name}:start -->");
    let end = format!("<!-- {name}:end -->");
    let (head, rest) = text.split_once(&start)?;
    let (_, tail) = rest.split_once(&end)?;
    Some(format!("{head}{}{tail}", block(name, body)))
}
fn read(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

fn fingerprint(context: &Context) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(CATALOG);
    hash.update(serde_json::to_vec(&metadata::native()).map_err(|e| e.to_string())?);
    hash.update(metadata::python_fingerprint(context)?);
    for path in [
        context.root.join("config/command-surface.json"),
        context.root.join("docs/cli"),
        context.root.join("docs/keybinds"),
    ] {
        hash_path(&path, &mut hash)?;
    }
    for input in doc_keybinds::INPUTS {
        hash_path(&context.root.join(input), &mut hash)?;
    }
    let catalog: Catalog = serde_json::from_str(CATALOG).map_err(|e| e.to_string())?;
    for name in catalog.native {
        if let Some(path) = metadata::binary(context, &name)?
            && let Ok(meta) = fs::metadata(&path)
        {
            hash.update(path.to_string_lossy().as_bytes());
            hash.update(meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified().and_then(|m| {
                m.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            }) {
                hash.update(modified.as_nanos().to_le_bytes());
            }
        }
    }
    Ok(format!("{:x}\n", hash.finalize()))
}
fn hash_path(path: &Path, hash: &mut Sha256) -> Result<(), String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    hash.update(path.to_string_lossy().as_bytes());
    if meta.is_dir() {
        let mut paths = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        paths.sort();
        for path in paths {
            hash_path(&path, hash)?;
        }
    } else if meta.is_file() {
        hash.update(fs::read(path).map_err(|e| e.to_string())?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_preserve_surrounding_text() {
        let text = "before\n<!-- cli:flags:start -->\nold\n<!-- cli:flags:end -->\nafter";
        assert_eq!(
            replace_block(text, "cli:flags", "new").unwrap(),
            text.replace("old", "new")
        );
    }
    #[test]
    fn unicode_tables_match_character_widths() {
        let table = table(&["Name", "Value"], &[vec!["π".into(), "x|y".into()]]);
        let lengths = table
            .lines()
            .map(|l| l.chars().count())
            .collect::<BTreeSet<_>>();
        assert_eq!(lengths.len(), 1);
        assert!(table.contains("x\\|y"));
    }
}
