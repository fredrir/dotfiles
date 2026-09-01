use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};

use super::config::{Configuration, Package, PackageKind, never_fold};

const PRUNE_DEPTH: usize = 6;

#[derive(Clone, Debug)]
enum Operation {
    Remove(PathBuf),
    CreateDirectory(PathBuf),
    Symlink {
        source: PathBuf,
        destination: PathBuf,
    },
}

#[derive(Clone, Debug)]
struct Item {
    action: Action,
    destination: PathBuf,
    detail: String,
    changed: bool,
}

#[derive(Clone, Debug)]
enum Node {
    Missing,
    File,
    Directory,
    Symlink(PathBuf),
}

#[derive(Default)]
struct VirtualFileSystem {
    nodes: BTreeMap<PathBuf, Node>,
    cleared: BTreeSet<PathBuf>,
}

impl VirtualFileSystem {
    fn inspect(&self, path: &Path) -> Result<Node, String> {
        if let Some(node) = self.nodes.get(path) {
            return Ok(node.clone());
        }
        if self.cleared.iter().any(|cleared| path.starts_with(cleared)) {
            return Ok(Node::Missing);
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(path)
                .map(|target| Node::Symlink(resolve_link(path, &target)))
                .map_err(|error| format!("read link {}: {error}", path.display())),
            Ok(metadata) if metadata.is_dir() => Ok(Node::Directory),
            Ok(_) => Ok(Node::File),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Node::Missing),
            Err(error) => Err(format!("read {}: {error}", path.display())),
        }
    }

    fn remove(&mut self, path: &Path) {
        self.nodes
            .retain(|candidate, _| !candidate.starts_with(path));
        self.cleared
            .retain(|candidate| !candidate.starts_with(path));
        self.cleared.insert(path.to_path_buf());
        self.nodes.insert(path.to_path_buf(), Node::Missing);
    }

    fn directory(&mut self, path: &Path) {
        self.nodes.insert(path.to_path_buf(), Node::Directory);
    }

    fn symlink(&mut self, path: &Path, source: &Path) {
        self.nodes
            .insert(path.to_path_buf(), Node::Symlink(source.to_path_buf()));
    }
}

#[derive(Default)]
pub struct LinkOutcome {
    pub checked: usize,
    pub changed: usize,
    pub links: usize,
    pub managed: Vec<PathBuf>,
}

pub fn synchronize(
    context: &Context,
    configuration: &Configuration,
    merge_paths: &HashSet<PathBuf>,
    dry_run: bool,
    events: &dyn EventSink,
) -> Result<LinkOutcome, String> {
    let linked_packages = configuration
        .packages
        .iter()
        .filter(|package| package.kind == PackageKind::Link)
        .count();
    events.emit(Event::PhaseStarted {
        phase: Phase::Plan,
        total: Some(linked_packages),
    });
    let mut planner = Planner {
        context,
        configuration,
        merge_paths,
        filesystem: VirtualFileSystem::default(),
        operations: Vec::new(),
        items: Vec::new(),
        conflicts: BTreeSet::new(),
        directories: BTreeSet::new(),
        managed: BTreeSet::new(),
        expansion: BTreeMap::new(),
    };
    planner.prune()?;
    let mut completed = 0;
    for package in &configuration.packages {
        if package.kind != PackageKind::Link {
            continue;
        }
        planner.walk_package(package)?;
        completed += 1;
        events.emit(Event::Progress {
            phase: Phase::Plan,
            completed,
            total: Some(linked_packages),
            label: package.name.clone(),
        });
    }
    if !planner.conflicts.is_empty() {
        for item in &planner.items {
            events.emit(Event::Item {
                action: item.action,
                path: item.destination.clone(),
                detail: item.detail.clone(),
                changed: item.changed,
            });
        }
        let first = planner.conflicts.first().expect("non-empty conflicts");
        events.emit(Event::Warning {
            message: format!(
                "{} unmanaged path{} block sync; first: {}",
                planner.conflicts.len(),
                if planner.conflicts.len() == 1 {
                    ""
                } else {
                    "s"
                },
                first.display()
            ),
            hint: Some(
                "move the conflicting paths aside and run dotfile sync again; use -v to list them"
                    .to_string(),
            ),
        });
        return Err(format!(
            "{} unmanaged conflict{}",
            planner.conflicts.len(),
            if planner.conflicts.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    events.emit(Event::PhaseStarted {
        phase: Phase::Links,
        total: Some(planner.operations.len()),
    });
    if dry_run {
        for (index, operation) in planner.operations.iter().enumerate() {
            emit_operation_progress(events, operation, index + 1, planner.operations.len());
        }
    } else {
        apply(&planner.operations, events)?;
    }
    for item in &planner.items {
        events.emit(Event::Item {
            action: item.action,
            path: item.destination.clone(),
            detail: item.detail.clone(),
            changed: item.changed,
        });
    }
    let changed = planner.items.iter().filter(|item| item.changed).count();
    let checked = planner.items.len();
    let links = planner
        .items
        .iter()
        .filter(|item| item.changed && item.action == Action::Link)
        .count();
    Ok(LinkOutcome {
        checked,
        changed,
        links,
        managed: planner.managed.into_iter().collect(),
    })
}

pub fn save_index(context: &Context, managed: &[PathBuf], dry_run: bool) -> Result<(), String> {
    if dry_run {
        return Ok(());
    }
    let content = managed
        .iter()
        .map(|path| format!("{}\n", path.display()))
        .collect::<String>();
    crate::context::write_atomic(&context.state.join("links"), content.as_bytes())
}

struct Planner<'a> {
    context: &'a Context,
    configuration: &'a Configuration,
    merge_paths: &'a HashSet<PathBuf>,
    filesystem: VirtualFileSystem,
    operations: Vec<Operation>,
    items: Vec<Item>,
    conflicts: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
    managed: BTreeSet<PathBuf>,
    expansion: BTreeMap<PathBuf, bool>,
}

impl Planner<'_> {
    fn prune(&mut self) -> Result<(), String> {
        for path in prune_candidates(self.context)? {
            let Node::Symlink(current) = self.filesystem.inspect(&path)? else {
                self.managed.remove(&path);
                continue;
            };
            if !owned_by_repo(self.context, &current) {
                self.managed.remove(&path);
                continue;
            }
            self.managed.insert(path.clone());
            if !stale_override_link(self.context, self.configuration, &current)?
                && target_exists(&path)?
            {
                continue;
            }
            self.remove(&path);
            self.items.push(Item {
                action: Action::Prune,
                destination: path,
                detail: "stale repository link".to_string(),
                changed: true,
            });
        }
        Ok(())
    }

    fn walk_package(&mut self, package: &Package) -> Result<(), String> {
        self.walk_node(package, Path::new(""), &package.directory, &package.name)
    }

    fn walk_node(
        &mut self,
        package: &Package,
        relative: &Path,
        source: &Path,
        full: &str,
    ) -> Result<(), String> {
        if self.source_is_filtered(source)? {
            return Ok(());
        }
        let destination =
            self.configuration
                .map_destination(self.context, full, &package.package, relative);
        let metadata = fs::symlink_metadata(source)
            .map_err(|error| format!("read {}: {error}", source.display()))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            self.link_directory(package, relative, source, full, &destination)?;
        } else {
            self.link_file(source, &destination)?;
        }
        Ok(())
    }

    fn link_file(&mut self, source: &Path, destination: &Path) -> Result<(), String> {
        match self.filesystem.inspect(destination)? {
            Node::Symlink(current) if current == source => self.items.push(Item {
                action: Action::Link,
                destination: destination.to_path_buf(),
                detail: "current".to_string(),
                changed: false,
            }),
            Node::Symlink(current) if owned_by_repo(self.context, &current) => {
                self.remove(destination);
                self.ensure_parent(destination);
                self.symlink(source, destination);
                self.items.push(Item {
                    action: Action::Link,
                    destination: destination.to_path_buf(),
                    detail: String::new(),
                    changed: true,
                });
            }
            Node::Missing => {
                self.ensure_parent(destination);
                self.symlink(source, destination);
                self.items.push(Item {
                    action: Action::Link,
                    destination: destination.to_path_buf(),
                    detail: String::new(),
                    changed: true,
                });
            }
            Node::Symlink(_) | Node::File | Node::Directory => {
                self.conflicts.insert(destination.to_path_buf());
                self.items.push(Item {
                    action: Action::Check,
                    destination: destination.to_path_buf(),
                    detail: "blocked by unmanaged path".to_string(),
                    changed: false,
                });
            }
        }
        Ok(())
    }

    fn link_directory(
        &mut self,
        package: &Package,
        relative: &Path,
        source: &Path,
        full: &str,
        destination: &Path,
    ) -> Result<(), String> {
        let must_expand = self.configuration.has_target_under(full)
            || self.merge_paths.iter().any(|path| path.starts_with(source))
            || self.has_filtered_descendant(source)?
            || generated_locally(self.context, source)?
            || self
                .context
                .root
                .join("linux/hyprland/elephant/files.toml")
                .starts_with(source);
        match self.filesystem.inspect(destination)? {
            Node::Symlink(current) if current == source => {
                if must_expand {
                    self.reset_fold(destination);
                } else {
                    self.items.push(Item {
                        action: Action::Link,
                        destination: destination.to_path_buf(),
                        detail: "current".to_string(),
                        changed: false,
                    });
                    return Ok(());
                }
            }
            Node::Symlink(current) if owned_by_repo(self.context, &current) => {
                if target_is_directory(&current)? {
                    self.unfold_preserving(destination, &current)?;
                } else {
                    self.remove(destination);
                }
            }
            Node::Symlink(_) => {
                self.conflicts.insert(destination.to_path_buf());
                self.items.push(Item {
                    action: Action::Check,
                    destination: destination.to_path_buf(),
                    detail: "blocked by unmanaged symlink".to_string(),
                    changed: false,
                });
                return Ok(());
            }
            Node::Missing => {
                if !must_expand && !never_fold(self.context, destination) {
                    self.ensure_parent(destination);
                    self.symlink(source, destination);
                    self.items.push(Item {
                        action: Action::Link,
                        destination: destination.to_path_buf(),
                        detail: String::new(),
                        changed: true,
                    });
                    return Ok(());
                }
            }
            Node::File => {
                self.conflicts.insert(destination.to_path_buf());
                self.items.push(Item {
                    action: Action::Check,
                    destination: destination.to_path_buf(),
                    detail: "blocked by unmanaged file".to_string(),
                    changed: false,
                });
                return Ok(());
            }
            Node::Directory => {}
        }
        for child in sorted_entries(source)? {
            let name = child
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    format!("repository path is not valid UTF-8: {}", child.display())
                })?;
            let child_relative = if relative.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                relative.join(name)
            };
            let child_full = format!("{full}/{name}");
            self.walk_node(package, &child_relative, &child, &child_full)?;
        }
        Ok(())
    }

    fn reset_fold(&mut self, destination: &Path) {
        self.remove(destination);
        self.mkdir(destination);
        self.items.push(Item {
            action: Action::Link,
            destination: destination.to_path_buf(),
            detail: "split layered directory".to_string(),
            changed: true,
        });
    }

    fn unfold_preserving(&mut self, destination: &Path, current: &Path) -> Result<(), String> {
        self.reset_fold(destination);
        self.preserve_directory(current, destination)
    }

    fn preserve_directory(&mut self, source: &Path, destination: &Path) -> Result<(), String> {
        for child in sorted_entries(source)? {
            if self.source_is_filtered(&child)? {
                continue;
            }
            let name = child
                .file_name()
                .ok_or_else(|| format!("repository path has no file name: {}", child.display()))?;
            let child_destination = destination.join(name);
            let metadata = fs::symlink_metadata(&child)
                .map_err(|error| format!("read {}: {error}", child.display()))?;
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && self.has_filtered_descendant(&child)?
            {
                self.mkdir(&child_destination);
                self.preserve_directory(&child, &child_destination)?;
            } else {
                self.symlink(&child, &child_destination);
            }
        }
        Ok(())
    }

    fn source_is_filtered(&self, source: &Path) -> Result<bool, String> {
        let basename = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("repository path is not valid UTF-8: {}", source.display()))?;
        Ok(matches!(
            basename,
            ".nolink" | ".secret" | ".system" | "merge.dotfile"
        ) || vault_owned(source)
            || generated_locally(self.context, source)?
            || self.merge_paths.contains(source))
    }

    fn has_filtered_descendant(&mut self, source: &Path) -> Result<bool, String> {
        if let Some(required) = self.expansion.get(source) {
            return Ok(*required);
        }
        let mut required = false;
        for child in sorted_entries(source)? {
            if self.source_is_filtered(&child)? {
                required = true;
                break;
            }
            let metadata = fs::symlink_metadata(&child)
                .map_err(|error| format!("read {}: {error}", child.display()))?;
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && self.has_filtered_descendant(&child)?
            {
                required = true;
                break;
            }
        }
        self.expansion.insert(source.to_path_buf(), required);
        Ok(required)
    }

    fn ensure_parent(&mut self, destination: &Path) {
        if let Some(parent) = destination.parent() {
            self.mkdir(parent);
        }
    }

    fn mkdir(&mut self, path: &Path) {
        if self.directories.insert(path.to_path_buf()) {
            self.operations
                .push(Operation::CreateDirectory(path.to_path_buf()));
        }
        self.filesystem.directory(path);
    }

    fn remove(&mut self, path: &Path) {
        self.operations.push(Operation::Remove(path.to_path_buf()));
        self.filesystem.remove(path);
        self.managed.retain(|managed| !managed.starts_with(path));
    }

    fn symlink(&mut self, source: &Path, destination: &Path) {
        self.operations.push(Operation::Symlink {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
        });
        self.filesystem.symlink(destination, source);
        self.managed.insert(destination.to_path_buf());
    }
}

fn apply(operations: &[Operation], events: &dyn EventSink) -> Result<(), String> {
    let mut replacing = false;
    for (index, operation) in operations.iter().enumerate() {
        if !replacing {
            crate::cancel::check()?;
        }
        match operation {
            Operation::Remove(path) => {
                fs::remove_file(path)
                    .map_err(|error| format!("remove {}: {error}", path.display()))?;
                replacing = true;
            }
            Operation::CreateDirectory(path) => fs::create_dir_all(path)
                .map_err(|error| format!("create {}: {error}", path.display()))?,
            Operation::Symlink {
                source,
                destination,
            } => {
                create_symlink(source, destination)?;
                replacing = false;
            }
        }
        emit_operation_progress(events, operation, index + 1, operations.len());
    }
    Ok(())
}

fn emit_operation_progress(
    events: &dyn EventSink,
    operation: &Operation,
    completed: usize,
    total: usize,
) {
    let path = match operation {
        Operation::Remove(path) | Operation::CreateDirectory(path) => path,
        Operation::Symlink { destination, .. } => destination,
    };
    events.emit(Event::Progress {
        phase: Phase::Links,
        completed,
        total: Some(total),
        label: path.display().to_string(),
    });
}

#[cfg(unix)]
fn create_symlink(source: &Path, destination: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(source, destination)
        .map_err(|error| format!("link {}: {error}", destination.display()))
}

#[cfg(not(unix))]
fn create_symlink(_source: &Path, _destination: &Path) -> Result<(), String> {
    Err("dotfile sync symlinks require Unix".to_string())
}

fn sorted_entries(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    let mut found = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("read {}: {error}", directory.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    found.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    Ok(found)
}

fn resolve_link(link: &Path, target: &Path) -> PathBuf {
    let combined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        link.parent().unwrap_or_else(|| Path::new("/")).join(target)
    };
    normalize(&combined)
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn owned_by_repo(context: &Context, path: &Path) -> bool {
    path != context.root && path.starts_with(&context.root)
}

fn vault_owned(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.ends_with(".enc") || name.contains(".enc.") || name.ends_with(".tmpl")
}

fn generated_locally(context: &Context, path: &Path) -> Result<bool, String> {
    if path == context.root.join("linux/hyprland/elephant/files.toml") {
        return Ok(true);
    }
    if path != context.root.join("linux/hyprland/hypr/conf.d/local.conf") {
        return Ok(false);
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    Ok(metadata.file_type().is_symlink() && !target_exists(path)?)
}

fn prune_candidates(context: &Context) -> Result<Vec<PathBuf>, String> {
    let index_path = context.state.join("links");
    match fs::read_to_string(&index_path) {
        Ok(index) => {
            return Ok(index
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("read {}: {error}", index_path.display())),
    }
    let mut found = BTreeSet::new();
    for path in sorted_entries(&context.home)? {
        if symlink_points_into(&path, &context.root)? {
            found.insert(path);
        }
    }
    for start in [context.home.join(".config"), context.home.join(".local")] {
        collect_repo_links(&start, 0, &context.root, &mut found)?;
    }
    Ok(found.into_iter().collect())
}

fn collect_repo_links(
    path: &Path,
    depth: usize,
    root: &Path,
    found: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        if symlink_points_into(path, root)? {
            found.insert(path.to_path_buf());
        }
        return Ok(());
    }
    if depth >= PRUNE_DEPTH || !metadata.is_dir() {
        return Ok(());
    }
    for child in sorted_entries(path)? {
        collect_repo_links(&child, depth + 1, root, found)?;
    }
    Ok(())
}

fn symlink_points_into(path: &Path, root: &Path) -> Result<bool, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }
    fs::read_link(path)
        .map(|target| target.is_absolute() && target.starts_with(root))
        .map_err(|error| format!("read link {}: {error}", path.display()))
}

fn stale_override_link(
    context: &Context,
    configuration: &Configuration,
    current: &Path,
) -> Result<bool, String> {
    let components = current.components().collect::<Vec<_>>();
    let Some(position) = components
        .iter()
        .position(|component| component.as_os_str() == "overrides")
    else {
        return Ok(false);
    };
    let mut base = PathBuf::new();
    for component in &components[..position] {
        base.push(component.as_os_str());
    }
    if !base.starts_with(&context.root) || !target_is_directory(&base.join("overrides"))? {
        return Ok(false);
    }
    Ok(!configuration
        .active_override_dirs
        .iter()
        .any(|active| current.starts_with(active)))
}

fn target_exists(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn target_is_directory(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}
