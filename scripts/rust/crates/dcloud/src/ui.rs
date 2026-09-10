use anyhow::Result;
use file_explorer::{
    AcceptTarget, Directory, DirectoryStatus, Entry, EntryKind, Explorer, FileSource, Outcome,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

struct Catalog {
    rows: Vec<Value>,
}
impl FileSource for Catalog {
    type Location = usize;
    type Error = std::io::Error;
    fn read_directory(&self, _: &usize) -> std::io::Result<Directory<usize>> {
        Ok(Directory {
            location: usize::MAX,
            parent: None,
            label: "dcloud · select a recovery point · type to filter".into(),
            status: DirectoryStatus::Present,
            entries: self
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| Entry {
                    location: index,
                    name: format!(
                        "{}  {} / {}  {}  {}",
                        field(row, "time"),
                        field(row, "host"),
                        field(row, "job"),
                        field(row, "destination"),
                        field(row, "id")
                    ),
                    kind: EntryKind::File,
                })
                .collect(),
        })
    }
}
fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn select(rows: &[Value]) -> Result<Option<usize>> {
    if rows.is_empty() {
        return Ok(None);
    }
    let style = workstation::Style::for_stdout();
    let outcome = Explorer::new(
        Catalog {
            rows: rows.to_vec(),
        },
        usize::MAX,
        &style,
    )
    .accept_target(AcceptTarget::HighlightedEntry)
    .selectable(|kind| kind == EntryKind::File)
    .run()?;
    Ok(match outcome {
        Outcome::Selected(selection) => Some(selection.location),
        _ => None,
    })
}

struct Tree {
    entries: Vec<(PathBuf, EntryKind)>,
}
impl FileSource for Tree {
    type Location = PathBuf;
    type Error = std::io::Error;
    fn read_directory(&self, location: &PathBuf) -> std::io::Result<Directory<PathBuf>> {
        let mut children = BTreeMap::<PathBuf, EntryKind>::new();
        for (path, kind) in &self.entries {
            let Ok(relative) = path.strip_prefix(location) else {
                continue;
            };
            let mut components = relative.components();
            let Some(component) = components.next() else {
                continue;
            };
            let child = location.join(component.as_os_str());
            let kind = if components.next().is_some() {
                EntryKind::Directory
            } else {
                *kind
            };
            children
                .entry(child)
                .and_modify(|v| {
                    if kind.is_directory() {
                        *v = kind;
                    }
                })
                .or_insert(kind);
        }
        Ok(Directory {
            location: location.clone(),
            parent: location.parent().map(Path::to_path_buf),
            label: format!("snapshot /{}", location.display()),
            status: DirectoryStatus::Present,
            entries: children
                .into_iter()
                .map(|(path, kind)| Entry {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    location: path,
                    kind,
                })
                .collect(),
        })
    }
}

pub fn tree(entries: &[Value]) -> Result<Option<PathBuf>> {
    let entries = entries
        .iter()
        .filter_map(|entry| {
            let path = entry.get("path")?.as_str()?;
            let path = PathBuf::from(path.trim_start_matches('/'));
            if path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            {
                return None;
            }
            let kind = match entry.get("type").and_then(Value::as_str) {
                Some("dir" | "directory") => EntryKind::Directory,
                Some("symlink") => EntryKind::Symlink,
                Some("file") => EntryKind::File,
                _ => EntryKind::Other,
            };
            Some((path, kind))
        })
        .collect();
    let style = workstation::Style::for_stdout();
    Ok(
        match Explorer::new(Tree { entries }, PathBuf::new(), &style).run()? {
            Outcome::Selected(s) => Some(s.location),
            _ => None,
        },
    )
}

pub fn safe(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                '�'
            } else {
                character
            }
        })
        .collect()
}

pub fn human(value: &Value) -> String {
    let mut output = String::new();
    render(value, 0, &mut output);
    output
}

fn render(value: &Value, depth: usize, output: &mut String) {
    let indent = "  ".repeat(depth);
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if value.is_null() {
                    continue;
                }
                if value.as_array().is_some_and(Vec::is_empty) {
                    if matches!(key.as_str(), "items" | "files" | "results") {
                        output.push_str(&format!("{indent}No {}.\n", safe(key)));
                    }
                    continue;
                }
                if value.is_object() || value.is_array() {
                    output.push_str(&format!("{indent}{}\n", safe(&key.replace('_', " "))));
                    render(value, depth + 1, output);
                } else if key == "log" {
                    for line in value.as_str().unwrap_or_default().lines() {
                        output.push_str(&format!("{indent}{}\n", safe(line)));
                    }
                } else {
                    output.push_str(&format!(
                        "{indent}{}: {}\n",
                        safe(&key.replace('_', " ")),
                        cell(value)
                    ));
                }
            }
            if object.is_empty() {
                output.push_str(&format!("{indent}No results.\n"));
            }
        }
        Value::Array(rows) => {
            if rows.is_empty() {
                output.push_str(&format!("{indent}No results.\n"));
            } else if let Some(columns) = columns(rows) {
                table(rows, &columns, depth, output);
            } else {
                for row in rows {
                    render(row, depth, output);
                }
            }
        }
        _ => output.push_str(&format!("{indent}{}\n", cell(value))),
    }
}

fn columns(rows: &[Value]) -> Option<Vec<&'static str>> {
    if !rows.iter().all(Value::is_object) {
        return None;
    }
    let first = &rows[0];
    let columns: &[&str] = if first.get("last_verified").is_some() {
        &[
            "host",
            "job",
            "destination",
            "status",
            "last_verified",
            "last_full_restore",
        ]
    } else if first.get("kind").is_some() && first.get("destination").is_some() {
        &[
            "kind",
            "host",
            "job",
            "destination",
            "time",
            "id",
            "category",
            "labels",
            "pinned",
            "cached_at",
        ]
    } else if first.get("path").is_some() {
        &["type", "kind", "size", "bytes", "path"]
    } else if first.get("pair").is_some() && first.get("status").is_some() {
        &["pair", "owner", "status", "last_success", "error"]
    } else {
        return None;
    };
    Some(
        columns
            .iter()
            .copied()
            .filter(|column| rows.iter().any(|row| !row[*column].is_null()))
            .collect(),
    )
}

fn table(rows: &[Value], columns: &[&str], depth: usize, output: &mut String) {
    let mut cells = vec![
        columns
            .iter()
            .map(|column| column.replace('_', " ").to_uppercase())
            .collect::<Vec<_>>(),
    ];
    cells.extend(
        rows.iter()
            .map(|row| columns.iter().map(|column| cell(&row[*column])).collect()),
    );
    let widths: Vec<_> = (0..columns.len())
        .map(|column| {
            cells
                .iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in cells {
        output.push_str(&"  ".repeat(depth));
        for (index, value) in row.iter().enumerate() {
            output.push_str(value);
            if index + 1 < row.len() {
                output
                    .push_str(&" ".repeat(widths[index].saturating_sub(value.chars().count()) + 2));
            }
        }
        output.push('\n');
    }
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "—".into(),
        Value::String(value) => safe(value),
        Value::Bool(true) => "yes".into(),
        Value::Bool(false) => "no".into(),
        Value::Array(values) => values.iter().map(cell).collect::<Vec<_>>().join(", "),
        _ => safe(&value.to_string()),
    }
}

#[cfg(test)]
#[path = "../tests/unit/ui_tests.rs"]
mod tests;
