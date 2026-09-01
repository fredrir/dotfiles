use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::Resolution;
use crate::context::{Context, write_atomic};
use crate::decision::{Choice, Client, Prompt};
use crate::event::{Action, Event, EventSink, Phase};

use super::config::{Configuration, PackageKind};

#[derive(Clone, Debug)]
enum LayerKind {
    Plain,
    Overlay,
}

#[derive(Clone, Debug)]
struct Layer {
    kind: LayerKind,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct LayerRecord {
    index: usize,
    kind: LayerKind,
    source: PathBuf,
    name: String,
    package_directory: PathBuf,
}

#[derive(Clone, Debug)]
struct AdoptionTarget {
    label: String,
    path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct MergeEntry {
    layers: Vec<Layer>,
    destination: PathBuf,
    ignores: Vec<String>,
    ignore_file: PathBuf,
    targets: Vec<AdoptionTarget>,
}

#[derive(Default)]
pub struct MergeOutcome {
    pub checked: usize,
    pub changed: usize,
    pub merges: usize,
    pub blocked: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ChangeKind {
    Add,
    Modify,
    Delete,
    Conflict,
}

#[derive(Clone, Debug)]
struct Change {
    kind: ChangeKind,
    path: Vec<String>,
    ours: Option<Value>,
    theirs: Option<Value>,
}

pub fn discover(
    context: &Context,
    configuration: &Configuration,
) -> Result<(Vec<MergeEntry>, HashSet<PathBuf>), String> {
    let mut records = BTreeMap::<(String, PathBuf), Vec<LayerRecord>>::new();
    for (index, package) in configuration.packages.iter().enumerate() {
        if package.kind != PackageKind::Link {
            continue;
        }
        let tag = package
            .name
            .rsplit_once('/')
            .map(|(group, _)| group.rsplit('/').next().unwrap_or(group))
            .unwrap_or_default();
        let files = package_files(&package.directory)?;
        for (relative, source) in &files {
            let overlay = overlay_target(relative, tag);
            let (key, kind) = match overlay {
                None => (relative.clone(), LayerKind::Plain),
                Some(base) => {
                    if files.contains_key(&base) {
                        return Err(format!(
                            "merge: {} carries both {} and overlay {}",
                            package.name,
                            base.display(),
                            relative.display()
                        ));
                    }
                    (base, LayerKind::Overlay)
                }
            };
            records
                .entry((package.package.clone(), key))
                .or_default()
                .push(LayerRecord {
                    index,
                    kind,
                    source: source.clone(),
                    name: package.name.clone(),
                    package_directory: package.directory.clone(),
                });
        }
    }
    let mut entries = Vec::new();
    let mut paths = HashSet::new();
    for ((package, base_relative), operations) in records {
        let overlays = operations
            .iter()
            .filter(|record| matches!(record.kind, LayerKind::Overlay))
            .collect::<Vec<_>>();
        if overlays.is_empty() {
            continue;
        }
        let providers = operations
            .iter()
            .filter(|record| matches!(record.kind, LayerKind::Plain))
            .collect::<Vec<_>>();
        if providers.is_empty() || providers[0].index > overlays[0].index {
            return Err(format!(
                "merge: overlay '{}/{}' has no {} in an earlier package of the same name",
                package,
                base_relative.display(),
                base_relative.display()
            ));
        }
        let base_group = providers[0]
            .name
            .rsplit_once('/')
            .map(|(group, _)| group)
            .unwrap_or_default();
        let full = format!("{base_group}/{package}/{}", base_relative.to_string_lossy());
        let destination = configuration.map_destination(context, &full, &package, &base_relative);
        let package_directories = operations
            .iter()
            .map(|record| record.package_directory.clone())
            .fold(Vec::<PathBuf>::new(), |mut found, directory| {
                if !found.contains(&directory) {
                    found.push(directory);
                }
                found
            });
        let ignores = load_ignores(&package_directories)?;
        let targets = adoption_targets(
            context,
            configuration,
            &package,
            &base_relative,
            providers[0].source.clone(),
            &operations,
        );
        let ignore_file = providers[0].package_directory.join("merge.dotfile");
        let layers = operations
            .into_iter()
            .map(|record| {
                paths.insert(record.source.clone());
                Layer {
                    kind: record.kind,
                    path: record.source,
                }
            })
            .collect();
        entries.push(MergeEntry {
            layers,
            destination,
            ignores,
            ignore_file,
            targets,
        });
    }
    entries.sort_by(|left, right| left.destination.cmp(&right.destination));
    Ok((entries, paths))
}

fn adoption_targets(
    context: &Context,
    configuration: &Configuration,
    package: &str,
    base_relative: &Path,
    shared: PathBuf,
    operations: &[LayerRecord],
) -> Vec<AdoptionTarget> {
    let mut targets = vec![AdoptionTarget {
        label: "shared".to_string(),
        path: shared,
    }];
    let mut seen = HashSet::from(["shared".to_string()]);
    for group in &configuration.groups {
        if group.contains("/overrides/") {
            continue;
        }
        let tag = group.rsplit('/').next().unwrap_or(group);
        if !seen.insert(tag.to_string()) {
            continue;
        }
        let existing = operations.iter().find(|record| {
            matches!(record.kind, LayerKind::Overlay)
                && record
                    .name
                    .rsplit_once('/')
                    .is_some_and(|(provider, _)| provider == group)
        });
        let path = existing.map_or_else(
            || {
                let file = base_relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("settings.json");
                let stem = file.strip_suffix(".json").unwrap_or(file);
                let mut relative = base_relative
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                relative.push(format!("{stem}.{tag}.json"));
                context.root.join(group).join(package).join(relative)
            },
            |record| record.source.clone(),
        );
        targets.push(AdoptionTarget {
            label: tag.to_string(),
            path,
        });
    }
    targets
}

pub fn synchronize(
    context: &Context,
    entries: &[MergeEntry],
    dry_run: bool,
    force: bool,
    resolution: Resolution,
    decisions: &Client,
    events: &dyn EventSink,
) -> Result<MergeOutcome, String> {
    events.emit(Event::PhaseStarted {
        phase: Phase::Merge,
        total: Some(entries.len()),
    });
    let mut outcome = MergeOutcome::default();
    let mut blocked = Vec::new();
    let mut selected = vec![EntryDecisions::default(); entries.len()];
    if !dry_run && !force && resolution == Resolution::Skip {
        for (index, entry) in entries.iter().enumerate() {
            for change in pending_changes(context, entry)? {
                let choice = decisions.choose(Prompt::Merge {
                    path: entry.destination.clone(),
                    key: if change.path.is_empty() {
                        "(document)".to_string()
                    } else {
                        change.path.join("/")
                    },
                    repo: shown(change.ours.as_ref()),
                    live: shown(change.theirs.as_ref()),
                })?;
                match choice {
                    Choice::Repo | Choice::Discard => {
                        selected[index]
                            .choices
                            .insert(change.path, AdoptionDecision::Repo);
                    }
                    Choice::Live => {
                        let default = default_target(entry, &change.path)?;
                        let target = if entry.targets.len() <= 1 {
                            0
                        } else {
                            match decisions.choose(Prompt::MergeTarget {
                                path: entry.destination.clone(),
                                key: if change.path.is_empty() {
                                    "(document)".to_string()
                                } else {
                                    change.path.join("/")
                                },
                                targets: entry
                                    .targets
                                    .iter()
                                    .map(|target| target.label.clone())
                                    .collect(),
                                default,
                            })? {
                                Choice::Target(target) if target < entry.targets.len() => target,
                                Choice::Live => default,
                                Choice::Abort | Choice::Cancel | Choice::Skip => {
                                    return Err("merge target selection cancelled".to_string());
                                }
                                _ => return Err("invalid merge target selection".to_string()),
                            }
                        };
                        selected[index]
                            .choices
                            .insert(change.path, AdoptionDecision::Live(target));
                    }
                    Choice::Ignore => {
                        selected[index]
                            .choices
                            .insert(change.path, AdoptionDecision::Ignore);
                    }
                    Choice::Skip => selected[index].skipped = true,
                    Choice::Abort | Choice::Cancel => {
                        return Err("merge decision cancelled".to_string());
                    }
                    Choice::Target(_) => return Err("invalid merge decision".to_string()),
                }
            }
        }
        for entry in &selected {
            for (path, choice) in &entry.choices {
                if *choice == AdoptionDecision::Ignore {
                    render_ignore_pattern(path)?;
                }
            }
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        crate::cancel::check()?;
        outcome.checked += 1;
        let result = settle(context, entry, dry_run, force, resolution, &selected[index])?;
        if result.changed {
            outcome.changed += 1;
            outcome.merges += 1;
        }
        if result.blocked {
            blocked.push(entry.destination.clone());
        }
        events.emit(Event::Item {
            action: Action::Merge,
            path: entry.destination.clone(),
            detail: result.detail,
            changed: result.changed,
        });
        events.emit(Event::Progress {
            phase: Phase::Merge,
            completed: index + 1,
            total: Some(entries.len()),
            label: entry.destination.display().to_string(),
        });
    }
    if blocked.is_empty() {
        Ok(outcome)
    } else {
        events.emit(Event::Warning {
            message: format!(
                "{} merged file{} need a decision; first: {}",
                blocked.len(),
                if blocked.len() == 1 { "" } else { "s" },
                blocked[0].display()
            ),
            hint: Some("use --resolve repo to discard live edits or --resolve live to adopt them; use -v for keys".to_string()),
        });
        outcome.blocked = blocked.len();
        Ok(outcome)
    }
}

#[derive(Clone, Default)]
struct EntryDecisions {
    choices: HashMap<Vec<String>, AdoptionDecision>,
    skipped: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdoptionDecision {
    Repo,
    Live(usize),
    Ignore,
}

struct SettleResult {
    changed: bool,
    blocked: bool,
    detail: String,
}

fn settle(
    context: &Context,
    entry: &MergeEntry,
    dry_run: bool,
    force: bool,
    resolution: Resolution,
    selected: &EntryDecisions,
) -> Result<SettleResult, String> {
    let ours = entry_document(entry)?;
    let metadata = match fs::symlink_metadata(&entry.destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("read {}: {error}", entry.destination.display())),
    };
    if metadata.as_ref().is_some_and(|value| value.is_dir()) {
        return Ok(SettleResult {
            changed: false,
            blocked: true,
            detail: "directory where a merged file belongs".to_string(),
        });
    }
    let is_link = metadata
        .as_ref()
        .is_some_and(|value| value.file_type().is_symlink());
    if is_link {
        let target = fs::read_link(&entry.destination)
            .map(|path| resolve_link(&entry.destination, &path))
            .map_err(|error| format!("read link {}: {error}", entry.destination.display()))?;
        if !target.starts_with(&context.root) && !force && resolution != Resolution::Repo {
            return Ok(SettleResult {
                changed: false,
                blocked: true,
                detail: "foreign symlink where a merged file belongs".to_string(),
            });
        }
    }
    if metadata.is_none() || is_link {
        if !dry_run {
            if is_link {
                fs::remove_file(&entry.destination)
                    .map_err(|error| format!("remove {}: {error}", entry.destination.display()))?;
            }
            write_document(&entry.destination, &ours)?;
            save_baseline(context, &entry.destination, &ours)?;
        }
        return Ok(SettleResult {
            changed: true,
            blocked: false,
            detail: if dry_run {
                "would materialize"
            } else {
                "materialized"
            }
            .to_string(),
        });
    }
    let raw = fs::read(&entry.destination)
        .map_err(|error| format!("read {}: {error}", entry.destination.display()))?;
    let live = std::str::from_utf8(&raw)
        .ok()
        .and_then(|text| parse_jsonc(text).ok());
    let Some(live) = live else {
        if !force && resolution != Resolution::Repo {
            return Ok(SettleResult {
                changed: false,
                blocked: true,
                detail: "not valid JSON".to_string(),
            });
        }
        if !dry_run {
            write_document(&entry.destination, &ours)?;
            save_baseline(context, &entry.destination, &ours)?;
        }
        return Ok(SettleResult {
            changed: true,
            blocked: false,
            detail: "restored invalid destination".to_string(),
        });
    };
    let baseline = load_baseline(context, &entry.destination)?;
    let mut changes = Vec::new();
    let no_repo_choices = HashSet::new();
    let document = Resolver {
        tracked: baseline.is_some(),
        ignores: &entry.ignores,
        repo_choices: &no_repo_choices,
    }
    .walk(
        Some(&ours),
        Some(&live),
        baseline.as_ref(),
        &[],
        &mut changes,
    )
    .unwrap_or_else(|| Value::Object(Map::new()));
    if !changes.is_empty()
        && !force
        && resolution == Resolution::Skip
        && (dry_run || selected.skipped || selected.choices.len() != changes.len())
    {
        return Ok(SettleResult {
            changed: false,
            blocked: true,
            detail: change_detail("drifted", &changes),
        });
    }
    let repo_choices = if force || resolution == Resolution::Repo {
        changes
            .iter()
            .map(|change| change.path.clone())
            .collect::<HashSet<_>>()
    } else if resolution == Resolution::Skip {
        selected
            .choices
            .iter()
            .filter_map(|(path, choice)| {
                (*choice == AdoptionDecision::Repo).then_some(path.clone())
            })
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let chosen = if !repo_choices.is_empty() {
        let mut ignored = Vec::new();
        Resolver {
            tracked: baseline.is_some(),
            ignores: &entry.ignores,
            repo_choices: &repo_choices,
        }
        .walk(
            Some(&ours),
            Some(&live),
            baseline.as_ref(),
            &[],
            &mut ignored,
        )
        .unwrap_or_else(|| Value::Object(Map::new()))
    } else {
        document
    };
    let live_changes = if resolution == Resolution::Live {
        changes
            .iter()
            .cloned()
            .map(|change| (change, None))
            .collect::<Vec<_>>()
    } else if resolution == Resolution::Skip {
        changes
            .iter()
            .filter_map(|change| match selected.choices.get(&change.path) {
                Some(AdoptionDecision::Live(target)) => Some((change.clone(), Some(*target))),
                _ => None,
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let ignored_changes = if resolution == Resolution::Skip {
        changes
            .iter()
            .filter(|change| selected.choices.get(&change.path) == Some(&AdoptionDecision::Ignore))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if !ignored_changes.is_empty() && !dry_run {
        add_ignores(&entry.ignore_file, &ignored_changes)?;
    }
    if !live_changes.is_empty() && !dry_run {
        adopt_changes(entry, &live_changes)?;
    }
    let rendered = render_json(&chosen)?;
    let destination_changed = chosen != live;
    let adopted = !live_changes.is_empty();
    let ignored = !ignored_changes.is_empty();
    if !dry_run {
        if destination_changed {
            write_atomic(&entry.destination, rendered.as_bytes())?;
        }
        save_baseline(context, &entry.destination, &chosen)?;
    }
    let detail = if ignored {
        change_detail("ignored", &ignored_changes)
    } else if adopted {
        change_detail("adopted", &changes)
    } else if destination_changed {
        "updated from repository".to_string()
    } else if raw != rendered.as_bytes() {
        "formatting preserved".to_string()
    } else {
        "current".to_string()
    };
    Ok(SettleResult {
        changed: destination_changed || adopted || ignored,
        blocked: false,
        detail,
    })
}

fn entry_document(entry: &MergeEntry) -> Result<Value, String> {
    let mut document = Value::Null;
    for layer in &entry.layers {
        let text = fs::read_to_string(&layer.path)
            .map_err(|error| format!("read {}: {error}", layer.path.display()))?;
        let parsed = parse_jsonc(&text).map_err(|error| {
            format!(
                "merge: {} is not valid JSONC: {error}",
                layer.path.display()
            )
        })?;
        document = match layer.kind {
            LayerKind::Plain => parsed,
            LayerKind::Overlay => deep_merge(document, parsed),
        };
    }
    Ok(document)
}

fn pending_changes(context: &Context, entry: &MergeEntry) -> Result<Vec<Change>, String> {
    let metadata = match fs::symlink_metadata(&entry.destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", entry.destination.display())),
    };
    if metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(Vec::new());
    }
    let raw = fs::read(&entry.destination)
        .map_err(|error| format!("read {}: {error}", entry.destination.display()))?;
    let Some(live) = std::str::from_utf8(&raw)
        .ok()
        .and_then(|text| parse_jsonc(text).ok())
    else {
        return Ok(Vec::new());
    };
    let ours = entry_document(entry)?;
    let baseline = load_baseline(context, &entry.destination)?;
    let mut changes = Vec::new();
    let repo_choices = HashSet::new();
    let _ = Resolver {
        tracked: baseline.is_some(),
        ignores: &entry.ignores,
        repo_choices: &repo_choices,
    }
    .walk(
        Some(&ours),
        Some(&live),
        baseline.as_ref(),
        &[],
        &mut changes,
    );
    Ok(changes)
}

fn default_target(entry: &MergeEntry, path: &[String]) -> Result<usize, String> {
    for (index, target) in entry.targets.iter().enumerate().rev() {
        let text = match fs::read_to_string(&target.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read {}: {error}", target.path.display())),
        };
        let document = parse_jsonc(&text).map_err(|error| {
            format!(
                "merge: {} is not valid JSONC: {error}",
                target.path.display()
            )
        })?;
        if value_at(&document, path).is_some() {
            return Ok(index);
        }
    }
    Ok(entry
        .layers
        .iter()
        .rev()
        .find(|layer| matches!(layer.kind, LayerKind::Overlay))
        .and_then(|layer| {
            entry
                .targets
                .iter()
                .position(|target| target.path == layer.path)
        })
        .unwrap_or(0))
}

fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                let merged = base
                    .remove(&key)
                    .map_or(value.clone(), |current| deep_merge(current, value));
                base.insert(key, merged);
            }
            Value::Object(base)
        }
        (_, overlay) => overlay,
    }
}

struct Resolver<'a> {
    tracked: bool,
    ignores: &'a [String],
    repo_choices: &'a HashSet<Vec<String>>,
}

impl Resolver<'_> {
    fn walk(
        &self,
        ours: Option<&Value>,
        theirs: Option<&Value>,
        baseline: Option<&Value>,
        path: &[String],
        changes: &mut Vec<Change>,
    ) -> Option<Value> {
        if !path.is_empty() && matches_ignore(path, self.ignores) {
            return theirs.cloned();
        }
        let branching = matches!(ours, Some(Value::Object(_)))
            && (matches!(theirs, Some(Value::Object(_))) || theirs.is_none())
            || ours.is_none() && matches!(theirs, Some(Value::Object(_)));
        if branching {
            let mut keys = Vec::new();
            if let Some(Value::Object(values)) = ours {
                keys.extend(values.keys().cloned());
            }
            if let Some(Value::Object(values)) = theirs {
                for key in values.keys() {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
            }
            let mut merged = Map::new();
            for key in keys {
                let ours_child = object_child(ours, &key);
                let theirs_child = object_child(theirs, &key);
                let baseline_child = object_child(baseline, &key);
                let mut child_path = path.to_vec();
                child_path.push(key.clone());
                if let Some(value) = self.walk(
                    ours_child,
                    theirs_child,
                    baseline_child,
                    &child_path,
                    changes,
                ) {
                    merged.insert(key, value);
                }
            }
            if !merged.is_empty()
                || matches!(ours, Some(Value::Object(values)) if values.is_empty())
                || matches!(theirs, Some(Value::Object(values)) if values.is_empty())
            {
                return Some(Value::Object(merged));
            }
            return None;
        }
        if ours == theirs {
            return ours.cloned();
        }
        let kind = if !self.tracked {
            Some(if ours.is_none() || theirs.is_none() {
                ChangeKind::Add
            } else {
                ChangeKind::Modify
            })
        } else if theirs.is_none() {
            baseline.is_some().then_some(ChangeKind::Delete)
        } else if ours.is_none() {
            baseline.is_none().then_some(ChangeKind::Add)
        } else if theirs == baseline {
            None
        } else if ours == baseline {
            Some(ChangeKind::Modify)
        } else {
            Some(ChangeKind::Conflict)
        };
        if let Some(kind) = kind {
            changes.push(Change {
                kind,
                path: path.to_vec(),
                ours: ours.cloned(),
                theirs: theirs.cloned(),
            });
            if self.repo_choices.contains(path) {
                ours.cloned()
            } else {
                theirs.cloned()
            }
        } else if ours.is_none() && baseline.is_some() {
            None
        } else {
            ours.cloned()
        }
    }
}

fn object_child<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match value {
        Some(Value::Object(values)) => values.get(key),
        _ => None,
    }
}

fn adopt_changes(entry: &MergeEntry, changes: &[(Change, Option<usize>)]) -> Result<(), String> {
    let mut documents = HashMap::<PathBuf, Value>::new();
    let mut texts = HashMap::<PathBuf, String>::new();
    let mut touched = HashSet::new();
    for layer in &entry.layers {
        let text = fs::read_to_string(&layer.path)
            .map_err(|error| format!("read {}: {error}", layer.path.display()))?;
        documents.insert(layer.path.clone(), parse_jsonc(&text)?);
        texts.insert(layer.path.clone(), text);
    }
    for target in &entry.targets {
        if documents.contains_key(&target.path) {
            continue;
        }
        let text = match fs::read_to_string(&target.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}\n".to_string(),
            Err(error) => return Err(format!("read {}: {error}", target.path.display())),
        };
        documents.insert(target.path.clone(), parse_jsonc(&text)?);
        texts.insert(target.path.clone(), text);
    }
    let base = entry
        .layers
        .iter()
        .find(|layer| matches!(layer.kind, LayerKind::Plain))
        .ok_or_else(|| "merge entry has no base layer".to_string())?
        .path
        .clone();
    for (change, selected_target) in changes {
        let owner = selected_target
            .and_then(|index| entry.targets.get(index))
            .map(|target| target.path.clone())
            .or_else(|| {
                entry
                    .layers
                    .iter()
                    .rev()
                    .find(|layer| {
                        documents
                            .get(&layer.path)
                            .is_some_and(|document| value_at(document, &change.path).is_some())
                    })
                    .map(|layer| layer.path.clone())
            })
            .unwrap_or_else(|| base.clone());
        let document = documents
            .get_mut(&owner)
            .ok_or_else(|| format!("missing merge layer {}", owner.display()))?;
        let text = texts
            .get_mut(&owner)
            .ok_or_else(|| format!("missing merge layer text {}", owner.display()))?;
        *text = if let Some(value) = &change.theirs {
            apply_jsonc_set(text, &change.path, value)?
        } else {
            apply_jsonc_remove(text, &change.path)?.unwrap_or_else(|| text.clone())
        };
        set_path(document, &change.path, change.theirs.clone());
        touched.insert(owner);
    }
    let mut paths = touched.into_iter().collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let text = texts.remove(&path).expect("path collected from map");
        let current = match fs::read_to_string(&path) {
            Ok(current) => current,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        if text != current {
            write_atomic(&path, text.as_bytes())?;
        }
    }
    Ok(())
}

fn value_at<'a>(document: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = document;
    for key in path {
        current = current.as_object()?.get(key)?;
    }
    Some(current)
}

fn set_path(document: &mut Value, path: &[String], value: Option<Value>) {
    if path.is_empty() {
        *document = value.unwrap_or_else(|| Value::Object(Map::new()));
        return;
    }
    let mut current = document;
    for key in &path[..path.len() - 1] {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        current = current
            .as_object_mut()
            .expect("object created above")
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        *current = Value::Object(Map::new());
    }
    let values = current.as_object_mut().expect("object created above");
    let key = path.last().expect("non-empty path").clone();
    if let Some(value) = value {
        values.insert(key, value);
    } else {
        values.remove(&key);
    }
}

fn package_files(directory: &Path) -> Result<BTreeMap<PathBuf, PathBuf>, String> {
    let mut files = BTreeMap::new();
    collect_package_files(directory, directory, &mut files)?;
    Ok(files)
}

fn collect_package_files(
    base: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", directory.display())),
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(".nolink" | ".secret" | ".system" | "merge.dotfile")
        ) || vault_owned(&path)
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_package_files(base, &path, files)?;
        } else if let Ok(relative) = path.strip_prefix(base) {
            files.insert(relative.to_path_buf(), path);
        }
    }
    Ok(())
}

fn overlay_target(relative: &Path, tag: &str) -> Option<PathBuf> {
    let text = relative.to_string_lossy();
    let suffix = format!(".{tag}.json");
    let stem = text.strip_suffix(&suffix)?;
    if stem.is_empty() || stem.ends_with('/') {
        None
    } else {
        Some(PathBuf::from(format!("{stem}.json")))
    }
}

fn load_ignores(directories: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut patterns = Vec::new();
    for directory in directories {
        let path = directory.join("merge.dotfile");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        for (offset, raw) in text.lines().enumerate() {
            let directive = raw.split('#').next().unwrap_or_default().trim();
            if directive.is_empty() {
                continue;
            }
            let Some(pattern) = directive.strip_prefix("ignore").map(str::trim) else {
                return Err(format!(
                    "{}:{}: expected 'ignore <pattern>', got '{directive}'",
                    path.display(),
                    offset + 1
                ));
            };
            if pattern.is_empty() {
                return Err(format!(
                    "{}:{}: expected 'ignore <pattern>', got '{directive}'",
                    path.display(),
                    offset + 1
                ));
            }
            if !patterns.iter().any(|existing| existing == pattern) {
                patterns.push(pattern.to_string());
            }
        }
    }
    Ok(patterns)
}

fn add_ignores(path: &Path, changes: &[Change]) -> Result<(), String> {
    let mut text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut existing = text
        .lines()
        .filter_map(|line| {
            line.split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .strip_prefix("ignore")
        })
        .map(str::trim)
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let gap = text
        .lines()
        .filter_map(|line| line.split('#').next())
        .find_map(|line| {
            let directive = line.trim();
            let rest = directive.strip_prefix("ignore")?;
            let width = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            (width > 0 && !rest.trim().is_empty()).then(|| rest[..width].to_string())
        })
        .unwrap_or_else(|| "  ".to_string());
    for change in changes {
        let pattern = render_ignore_pattern(&change.path)?;
        if !existing.insert(pattern.clone()) {
            continue;
        }
        if !text.is_empty() && !text.ends_with(['\n', '\r']) {
            text.push_str(newline);
        }
        text.push_str("ignore");
        text.push_str(&gap);
        text.push_str(&pattern);
        text.push_str(newline);
    }
    write_atomic(path, text.as_bytes())
}

fn render_ignore_pattern(path: &[String]) -> Result<String, String> {
    if path.is_empty() {
        return Err("cannot ignore an entire merged document".to_string());
    }
    for key in path {
        if key.is_empty() || key.contains(['/', '#', '\n', '\r']) || key.trim() != key {
            return Err(format!(
                "key '{key}' cannot be represented in merge.dotfile"
            ));
        }
    }
    Ok(path.join("/"))
}

fn matches_ignore(path: &[String], patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        let segments = pattern.split('/').collect::<Vec<_>>();
        segments.len() <= path.len()
            && segments
                .iter()
                .zip(path)
                .all(|(pattern, value)| wildcard_match(pattern, value))
    })
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    fn matches(pattern: &[char], value: &[char]) -> bool {
        match pattern {
            [] => value.is_empty(),
            ['*', rest @ ..] => {
                matches(rest, value) || !value.is_empty() && matches(pattern, &value[1..])
            }
            ['?', rest @ ..] => !value.is_empty() && matches(rest, &value[1..]),
            [first, rest @ ..] => value.first() == Some(first) && matches(rest, &value[1..]),
        }
    }
    matches(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn parse_jsonc(text: &str) -> Result<Value, String> {
    let bytes = text.as_bytes();
    let mut clean = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if string {
            clean.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            string = true;
            clean.push(byte);
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else {
            clean.push(byte);
            index += 1;
        }
    }
    let mut without_trailing = Vec::with_capacity(clean.len());
    let mut index = 0;
    let mut string = false;
    let mut escaped = false;
    while index < clean.len() {
        let byte = clean[index];
        if string {
            without_trailing.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            string = true;
        }
        if byte == b',' {
            let mut next = index + 1;
            while clean.get(next).is_some_and(u8::is_ascii_whitespace) {
                next += 1;
            }
            if matches!(clean.get(next), Some(b'}' | b']')) {
                index += 1;
                continue;
            }
        }
        without_trailing.push(byte);
        index += 1;
    }
    serde_json::from_slice(&without_trailing).map_err(|error| error.to_string())
}

fn render_json(document: &Value) -> Result<String, String> {
    let mut output = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    document
        .serialize(&mut serializer)
        .map_err(|error| error.to_string())?;
    output.push(b'\n');
    String::from_utf8(output).map_err(|error| error.to_string())
}

fn write_document(path: &Path, document: &Value) -> Result<(), String> {
    write_atomic(path, render_json(document)?.as_bytes())
}

fn baseline_path(context: &Context, destination: &Path) -> PathBuf {
    let digest = sha256(destination.to_string_lossy().as_bytes());
    context.state.join("merge").join(format!("{digest}.json"))
}

fn load_baseline(context: &Context, destination: &Path) -> Result<Option<Value>, String> {
    let path = baseline_path(context, destination);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("read {}: {error}", path.display()))
}

fn save_baseline(context: &Context, destination: &Path, document: &Value) -> Result<(), String> {
    write_document(&baseline_path(context, destination), document)
}

fn change_detail(prefix: &str, changes: &[Change]) -> String {
    let mut labels = changes
        .iter()
        .take(5)
        .map(|change| {
            let kind = match change.kind {
                ChangeKind::Add => "add",
                ChangeKind::Modify => "modify",
                ChangeKind::Delete => "delete",
                ChangeKind::Conflict => "conflict",
            };
            format!(
                "{kind}:{} {} → {}",
                if change.path.is_empty() {
                    "(document)".to_string()
                } else {
                    change.path.join("/")
                },
                shown(change.ours.as_ref()),
                shown(change.theirs.as_ref())
            )
        })
        .collect::<Vec<_>>();
    if changes.len() > 5 {
        labels.push(format!("+{} more", changes.len() - 5));
    }
    format!("{prefix} {}", labels.join(", "))
}

fn shown(value: Option<&Value>) -> String {
    value
        .map(|value| serde_json::to_string(value).unwrap_or_else(|_| "?".to_string()))
        .unwrap_or_else(|| "∅".to_string())
}

#[derive(Clone)]
struct JsonMember {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

fn apply_jsonc_set(text: &str, path: &[String], value: &Value) -> Result<String, String> {
    if path.is_empty() {
        return Err("cannot adopt an empty JSON path".to_string());
    }
    let unit = detect_indent(text);
    let newline = if text
        .find('\n')
        .is_some_and(|index| index > 0 && text.as_bytes()[index - 1] == b'\r')
    {
        "\r\n"
    } else {
        "\n"
    };
    if let Some((start, end)) = json_value_span(text, path)? {
        let rendered = render_fragment(value, &unit, &line_indent(text, start), newline)?;
        return Ok(format!("{}{}{}", &text[..start], rendered, &text[end..]));
    }
    let (body, missing) = json_target(text, path)?;
    let nested = nest_value(&missing[1..], value.clone());
    insert_json_member(text, body, &missing[0], &nested, &unit, newline)
}

fn apply_jsonc_remove(text: &str, path: &[String]) -> Result<Option<String>, String> {
    if path.is_empty() {
        return Err("cannot remove an empty JSON path".to_string());
    }
    let Some(body) = json_container_span(text, path)? else {
        return Ok(None);
    };
    let items = json_members(text, body)?;
    let Some(index) = items
        .iter()
        .rposition(|item| item.key == path[path.len() - 1])
    else {
        return Ok(None);
    };
    let mut start = items[index].key_start;
    let mut end = items[index].value_end;
    let mut comma = None;
    let probe = skip_json_blanks(text, end)?;
    if probe < body.1 && text.as_bytes().get(probe) == Some(&b',') {
        end = probe + 1;
    } else if index > 0 {
        let prior = skip_json_blanks(text, items[index - 1].value_end)?;
        if text.as_bytes().get(prior) == Some(&b',') {
            comma = Some(prior);
        }
    }
    let head = text[..start].rfind('\n').map_or(0, |offset| offset + 1);
    if text[head..start].trim().is_empty() {
        start = head;
    }
    if let Some(line) = text[end..].find('\n').map(|offset| end + offset) {
        let rest = &text[end..line];
        if rest.trim().is_empty() || rest.trim_start().starts_with("//") {
            end = line + 1;
        }
    }
    Ok(Some(match comma {
        None => format!("{}{}", &text[..start], &text[end..]),
        Some(comma) => format!(
            "{}{}{}",
            &text[..comma],
            &text[comma + 1..start],
            &text[end..]
        ),
    }))
}

fn json_target(text: &str, path: &[String]) -> Result<((usize, usize), Vec<String>), String> {
    let mut body = json_root_body(text)?
        .ok_or_else(|| "the document root is not a JSON object".to_string())?;
    for depth in 0..path.len() - 1 {
        let prefix = &path[..depth + 1];
        if let Some(inner) = json_descend(text, prefix)? {
            body = inner;
        } else {
            if json_key_span(text, prefix)?.is_some() {
                return Err(format!("'{}' is not an object", path[depth]));
            }
            return Ok((body, path[depth..].to_vec()));
        }
    }
    Ok((body, path[path.len() - 1..].to_vec()))
}

fn insert_json_member(
    text: &str,
    body: (usize, usize),
    key: &str,
    value: &Value,
    unit: &str,
    newline: &str,
) -> Result<String, String> {
    let items = json_members(text, body)?;
    if items.is_empty() {
        let outer = line_indent(text, body.1);
        let indent = format!("{outer}{unit}");
        let member = render_member(key, value, unit, &indent, newline)?;
        if text[body.0..body.1].contains('\n') {
            let at = text[..body.1]
                .rfind('\n')
                .map_or(body.0, |offset| offset + 1);
            return Ok(format!(
                "{}{}{}{}{}",
                &text[..at],
                indent,
                member,
                newline,
                &text[at..]
            ));
        }
        return Ok(format!(
            "{}{}{}{}{}{}{}",
            &text[..body.1],
            newline,
            indent,
            member,
            newline,
            outer,
            &text[body.1..]
        ));
    }
    let last = items.last().expect("non-empty items");
    let mut tail = last.value_end;
    let probe = skip_json_blanks(text, tail)?;
    let comma = probe < body.1 && text.as_bytes().get(probe) == Some(&b',');
    let lead = if comma { "" } else { "," };
    if comma {
        tail = probe + 1;
    }
    let cut = text[tail..].find('\n').map(|offset| tail + offset);
    if cut.is_none_or(|cut| cut > body.1) {
        let flat = format!(
            "{}: {}",
            serde_json::to_string(key).map_err(|error| error.to_string())?,
            serde_json::to_string(value).map_err(|error| error.to_string())?
        );
        return Ok(format!(
            "{}{} {}{}",
            &text[..tail],
            lead,
            flat,
            &text[tail..]
        ));
    }
    let mut cut = cut.expect("checked above");
    if cut > 0 && text.as_bytes()[cut - 1] == b'\r' {
        cut -= 1;
    }
    let indent = line_indent(text, last.key_start);
    let member = render_member(key, value, unit, &indent, newline)?;
    Ok(format!(
        "{}{}{}{}{}{}{}",
        &text[..tail],
        lead,
        &text[tail..cut],
        newline,
        indent,
        member,
        &text[cut..]
    ))
}

fn render_member(
    key: &str,
    value: &Value,
    unit: &str,
    indent: &str,
    newline: &str,
) -> Result<String, String> {
    Ok(format!(
        "{}: {}",
        serde_json::to_string(key).map_err(|error| error.to_string())?,
        render_fragment(value, unit, indent, newline)?
    ))
}

fn render_fragment(
    value: &Value,
    unit: &str,
    indent: &str,
    newline: &str,
) -> Result<String, String> {
    let mut output = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|error| error.to_string())?;
    let rendered = String::from_utf8(output).map_err(|error| error.to_string())?;
    Ok(rendered.replace('\n', &format!("{newline}{indent}")))
}

fn nest_value(keys: &[String], mut value: Value) -> Value {
    for key in keys.iter().rev() {
        let mut object = Map::new();
        object.insert(key.clone(), value);
        value = Value::Object(object);
    }
    value
}

fn json_value_span(text: &str, path: &[String]) -> Result<Option<(usize, usize)>, String> {
    let Some(body) = json_descend(text, &path[..path.len() - 1])? else {
        return Ok(None);
    };
    Ok(json_find(text, body, &path[path.len() - 1])?
        .map(|member| (member.value_start, member.value_end)))
}

fn json_key_span(text: &str, path: &[String]) -> Result<Option<(usize, usize)>, String> {
    if path.is_empty() {
        return Ok(None);
    }
    let Some(body) = json_descend(text, &path[..path.len() - 1])? else {
        return Ok(None);
    };
    Ok(json_find(text, body, &path[path.len() - 1])?
        .map(|member| (member.key_start, member.value_end)))
}

fn json_container_span(text: &str, path: &[String]) -> Result<Option<(usize, usize)>, String> {
    if path.is_empty() {
        return Ok(None);
    }
    json_descend(text, &path[..path.len() - 1])
}

fn json_descend(text: &str, path: &[String]) -> Result<Option<(usize, usize)>, String> {
    let Some(mut body) = json_root_body(text)? else {
        return Ok(None);
    };
    for key in path {
        let Some(member) = json_find(text, body, key)? else {
            return Ok(None);
        };
        if text.as_bytes().get(member.value_start) != Some(&b'{') {
            return Ok(None);
        }
        body = (member.value_start + 1, member.value_end - 1);
    }
    Ok(Some(body))
}

fn json_root_body(text: &str) -> Result<Option<(usize, usize)>, String> {
    let index = skip_json_blanks(text, 0)?;
    if text.as_bytes().get(index) != Some(&b'{') {
        return Ok(None);
    }
    Ok(Some((index + 1, scan_json_container(text, index)? - 1)))
}

fn json_find(text: &str, body: (usize, usize), key: &str) -> Result<Option<JsonMember>, String> {
    Ok(json_members(text, body)?
        .into_iter()
        .rfind(|member| member.key == key))
}

fn json_members(text: &str, body: (usize, usize)) -> Result<Vec<JsonMember>, String> {
    let mut found = Vec::new();
    let mut index = body.0;
    loop {
        index = skip_json_blanks(text, index)?;
        if index >= body.1 {
            return Ok(found);
        }
        if text.as_bytes()[index] == b',' {
            index += 1;
            continue;
        }
        if text.as_bytes()[index] != b'"' {
            return Err(format!("expected a key at offset {index}"));
        }
        let key_start = index;
        let (key, after) = scan_json_string(text, index)?;
        let after = skip_json_blanks(text, after)?;
        if after >= body.1 || text.as_bytes()[after] != b':' {
            return Err(format!("expected ':' after the key at offset {index}"));
        }
        let value_start = skip_json_blanks(text, after + 1)?;
        let value_end = scan_json_value(text, value_start)?;
        if value_end == value_start {
            return Err(format!("expected a value at offset {value_start}"));
        }
        found.push(JsonMember {
            key,
            key_start,
            value_start,
            value_end,
        });
        index = value_end;
    }
}

fn skip_json_blanks(text: &str, mut index: usize) -> Result<usize, String> {
    let bytes = text.as_bytes();
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index..].starts_with(b"//") {
            index = text[index..]
                .find('\n')
                .map_or(bytes.len(), |offset| index + offset);
        } else if bytes[index..].starts_with(b"/*") {
            let end = text[index + 2..]
                .find("*/")
                .ok_or_else(|| "unterminated block comment".to_string())?;
            index += end + 4;
        } else {
            break;
        }
    }
    Ok(index)
}

fn scan_json_string(text: &str, index: usize) -> Result<(String, usize), String> {
    let bytes = text.as_bytes();
    let mut probe = index + 1;
    while probe < bytes.len() {
        if bytes[probe] == b'\\' {
            probe += 2;
        } else if bytes[probe] == b'"' {
            let key =
                serde_json::from_str(&text[index..=probe]).map_err(|error| error.to_string())?;
            return Ok((key, probe + 1));
        } else {
            probe += 1;
        }
    }
    Err(format!("unterminated string at offset {index}"))
}

fn scan_json_container(text: &str, mut index: usize) -> Result<usize, String> {
    let bytes = text.as_bytes();
    let close = if bytes[index] == b'{' { b'}' } else { b']' };
    let mut depth = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            index = scan_json_string(text, index)?.1;
            continue;
        }
        if bytes[index..].starts_with(b"//") || bytes[index..].starts_with(b"/*") {
            index = skip_json_blanks(text, index)?;
            continue;
        }
        if matches!(bytes[index], b'{' | b'[') {
            depth += 1;
        } else if matches!(bytes[index], b'}' | b']') {
            depth -= 1;
            if depth == 0 {
                if bytes[index] != close {
                    return Err(format!("mismatched bracket at offset {index}"));
                }
                return Ok(index + 1);
            }
        }
        index += 1;
    }
    Err("unterminated container".to_string())
}

fn scan_json_value(text: &str, index: usize) -> Result<usize, String> {
    let Some(byte) = text.as_bytes().get(index) else {
        return Ok(index);
    };
    if *byte == b'"' {
        return Ok(scan_json_string(text, index)?.1);
    }
    if matches!(*byte, b'{' | b'[') {
        return scan_json_container(text, index);
    }
    let mut end = index;
    while text
        .as_bytes()
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"+-._".contains(byte))
    {
        end += 1;
    }
    Ok(end)
}

fn detect_indent(text: &str) -> String {
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            return "\t".to_string();
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        if spaces > 0 {
            return " ".repeat(spaces);
        }
    }
    "\t".to_string()
}

fn line_indent(text: &str, offset: usize) -> String {
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    text[start..]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

fn resolve_link(link: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        link.parent().unwrap_or_else(|| Path::new("/")).join(target)
    }
}

fn vault_owned(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.ends_with(".enc") || name.contains(".enc.") || name.ends_with(".tmpl")
}

fn sha256(input: &[u8]) -> String {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_length = (input.len() as u64) * 8;
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    let mut hash = INITIAL;
    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four bytes"));
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        }
        for (slot, value) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    hash.iter().map(|value| format!("{value:08x}")).collect()
}
