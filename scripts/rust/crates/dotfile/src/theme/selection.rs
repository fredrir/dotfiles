use super::{Result, emitters::Target, model::Repository};
use crate::config::blocks;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};
#[derive(Clone, Debug)]
pub struct Selection {
    pub groups: BTreeMap<String, BTreeMap<String, String>>,
}
pub fn group(path: &str) -> String {
    let mut p = path.split('/');
    let first = p.next().unwrap_or("");
    if first == "linux" {
        format!("linux/{}", p.next().unwrap_or(""))
    } else {
        first.into()
    }
}
pub fn package(path: &str) -> String {
    path.get(group(path).len() + 1..)
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .into()
}
pub fn inventory(targets: &[Target]) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for target in targets {
        map.entry(group(&target.path))
            .or_default()
            .insert(package(&target.path));
    }
    map.into_iter()
        .map(|(g, p)| (g, p.into_iter().collect()))
        .collect()
}
impl Selection {
    pub fn load(repo: &Repository, targets: &[Target]) -> Result<Self> {
        let source = fs::read_to_string(repo.root.join("config/profiles.dotfile"))
            .map_err(|e| format!("config/profiles.dotfile: {e}"))?;
        Self::parse(repo, targets, &source)
    }
    pub fn parse(repo: &Repository, targets: &[Target], source: &str) -> Result<Self> {
        let entries = blocks::parse(source).map_err(|e| format!("config/profiles.dotfile {e}"))?;
        let mut groups: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut errors = Vec::new();
        for entry in entries {
            let keys = groups.entry(entry.block.clone()).or_default();
            if entry.opens {
                continue;
            }
            let (key, value) = entry.split();
            let value = value.trim_matches(['\'', '"']);
            if value.is_empty() {
                errors.push(format!("line {}: '{key}' has no profile", entry.number));
            } else if !repo.themes.contains_key(value) {
                errors.push(format!("line {}: unknown profile '{value}'", entry.number));
            } else {
                keys.insert(key.into(), value.into());
            }
        }
        let inventory = inventory(targets);
        for (group, values) in &groups {
            let Some(packages) = inventory.get(group) else {
                errors.push(format!("group '{group}' owns no generated file"));
                continue;
            };
            for key in values.keys() {
                if key != "theme" && !packages.contains(key) {
                    errors.push(format!("group '{group}' has no '{key}' output to theme"));
                }
            }
        }
        if !groups
            .get("shared")
            .is_some_and(|g| g.contains_key("theme"))
        {
            errors.push("'shared' must set a 'theme', it is the fallback".into());
        }
        if errors.is_empty() {
            Ok(Self { groups })
        } else {
            Err(format!(
                "config/profiles.dotfile is not usable:\n  {}",
                errors.join("\n  ")
            ))
        }
    }
    pub fn default(&self) -> &str {
        &self.groups["shared"]["theme"]
    }
    pub fn current(&self, group: &str, key: &str) -> &str {
        self.groups
            .get(group)
            .and_then(|g| g.get(key).or_else(|| g.get("theme")))
            .map(String::as_str)
            .unwrap_or_else(|| self.default())
    }
    pub fn for_path(&self, path: &str) -> &str {
        self.current(&group(path), &package(path))
    }
    pub fn scope_of(&self, path: &str) -> String {
        let g = group(path);
        let p = package(path);
        if self
            .groups
            .get(&g)
            .is_some_and(|keys| keys.contains_key(&p))
        {
            format!("{g}/{p}")
        } else {
            g
        }
    }
    pub fn overrides(&self) -> Vec<(String, String)> {
        self.groups
            .iter()
            .flat_map(|(g, keys)| {
                keys.keys()
                    .filter(move |k| g != "shared" || k.as_str() != "theme")
                    .map(move |k| (g.clone(), k.clone()))
            })
            .collect()
    }
}
pub fn scope(scope: &str, targets: &[Target]) -> Result<(String, String, bool)> {
    let scope = scope.trim_matches('/');
    if scope == "global" {
        return Ok(("shared".into(), "theme".into(), true));
    }
    let inventory = inventory(targets);
    if inventory.contains_key(scope) {
        return Ok((scope.into(), "theme".into(), false));
    }
    if let Some((group, package)) = scope.rsplit_once('/')
        && inventory
            .get(group)
            .is_some_and(|p| p.iter().any(|p| p == package))
    {
        return Ok((group.into(), package.into(), false));
    }
    Err(format!(
        "nothing generated is scoped to '{scope}' (groups: {})",
        inventory.keys().cloned().collect::<Vec<_>>().join(", ")
    ))
}
fn code(line: &str) -> &str {
    line.split('#').next().unwrap_or("").trim()
}
fn spans(lines: &[String]) -> BTreeMap<String, (usize, usize)> {
    let mut result = BTreeMap::new();
    let mut current = None;
    for (i, line) in lines.iter().enumerate() {
        let body = code(line);
        if body == "}" {
            if let Some((name, start)) = current.take() {
                result.insert(name, (start, i));
            }
        } else if let Some(name) = body.strip_suffix('{') {
            current = Some((name.trim().to_string(), i));
        }
    }
    result
}
fn assign(source: &str, group: &str, key: &str, value: &str) -> String {
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let positions = spans(&lines);
    let Some(&(start, end)) = positions.get(group) else {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.extend([
            format!("{group} {{"),
            format!("  {key} = {value}"),
            "}".into(),
        ]);
        return lines.join("\n") + "\n";
    };
    let entries = (start + 1..end)
        .filter(|i| !code(&lines[*i]).is_empty())
        .collect::<Vec<_>>();
    for i in &entries {
        if code(&lines[*i]).split('=').next().unwrap_or("").trim() == key {
            let old = &lines[*i];
            let (comment_at, comment) = old.find('#').map_or((old.len(), ""), |p| (p, &old[p..]));
            let body = &old[..comment_at];
            let (head, tail) = body.split_once('=').unwrap_or((body, ""));
            let lead = &tail[..tail.len() - tail.trim_start().len()];
            let lead = if lead.is_empty() { " " } else { lead };
            let trail = &tail[tail.trim_end().len()..];
            let updated = format!("{head}={lead}{value}{trail}{comment}");
            if updated == *old {
                return source.to_string();
            }
            lines[*i] = updated;
            return lines.join("\n") + "\n";
        }
    }
    let indent = entries
        .first()
        .map(|i| &lines[*i][..lines[*i].len() - lines[*i].trim_start().len()])
        .filter(|s| !s.is_empty())
        .unwrap_or("  ");
    lines.insert(end, format!("{indent}{key} = {value}"));
    lines.join("\n") + "\n"
}
fn remove(source: &str, group: &str, key: &str) -> String {
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let positions = spans(&lines);
    if let Some(&(mut start, end)) = positions.get(group) {
        let entries = (start + 1..end)
            .filter(|i| !code(&lines[*i]).is_empty())
            .collect::<Vec<_>>();
        if let Some(index) = entries
            .iter()
            .find(|i| code(&lines[**i]).split('=').next().unwrap_or("").trim() == key)
        {
            if entries.len() > 1 {
                lines.remove(*index);
            } else {
                while start > 0 && lines[start - 1].trim().is_empty() {
                    start -= 1;
                }
                lines.drain(start..=end);
            }
        }
    }
    lines.join("\n") + "\n"
}
pub fn switched(
    source: &str,
    selection: &Selection,
    group: &str,
    key: &str,
    global: bool,
    profile: &str,
) -> String {
    let mut text = source.to_string();
    if global {
        for (g, k) in selection.overrides() {
            text = remove(&text, &g, &k);
        }
    }
    assign(&text, group, key, profile)
}
