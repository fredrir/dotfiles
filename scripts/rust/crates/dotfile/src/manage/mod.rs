//! Adopt and release tracked configuration with rollback on failure.
use crate::artifacts::packages;
use crate::config::{Configuration, never_fold};
use crate::context::Context;
use clap::Args;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

pub use crate::fs::transaction;
use transaction::Transaction;

#[derive(Debug, Args)]
pub struct AddArgs {
    pub path: PathBuf,
    #[arg(long)]
    pub shared: bool,
    #[arg(long)]
    pub linux: bool,
    #[arg(long)]
    pub arch: bool,
    #[arg(long)]
    pub ubuntu: bool,
    #[arg(long)]
    pub kde: bool,
    #[arg(long)]
    pub hyprland: bool,
    #[arg(long)]
    pub server: bool,
    #[arg(long)]
    pub macos: bool,
    #[arg(long)]
    pub pkg: Option<String>,
    #[arg(long, alias = "desc")]
    pub description: Option<String>,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    pub path: PathBuf,
}

pub fn validate_group(group: &str) -> Result<(), String> {
    packages::validate_group(group)?;
    safe_relative(Path::new(group)).map(|_| ())
}
pub fn validate_package(package: &str) -> Result<(), String> {
    packages::validate_package(package)?;
    if matches!(package, "." | ".." | "overrides") {
        return Err(format!("invalid package: {package}"));
    }
    Ok(())
}

fn safe_relative(path: &Path) -> Result<PathBuf, String> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "path must be relative and cannot contain parent traversal: {}",
                    path.display()
                ));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("path must include a package".into());
    }
    Ok(clean)
}

pub fn expand(context: &Context, path: &Path) -> Result<PathBuf, String> {
    let path = match path.to_str() {
        Some("~") => context.home.clone(),
        Some(value) if value.starts_with("~/") => context.home.join(&value[2..]),
        _ if path.is_absolute() => path.into(),
        _ => std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path),
    };
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            part => clean.push(part.as_os_str()),
        }
    }
    Ok(clean)
}

pub fn require_within(root: &Path, path: &Path) -> Result<(), String> {
    let root = crate::fs::resolved(root)?;
    let path = crate::fs::resolved(path)?;
    if !path.starts_with(&root) || path == root {
        return Err(format!(
            "path escapes its allowed directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn config(context: &Context) -> Result<Configuration, String> {
    Ok(Configuration {
        targets: crate::config::load_targets(context)?,
        groups: Vec::new(),
        active_override_dirs: Vec::new(),
        overrides: BTreeMap::new(),
        packages: Vec::new(),
    })
}

fn locate_source(context: &Context, path: &Path) -> Result<PathBuf, String> {
    let full = expand(context, path)?;
    if fs::symlink_metadata(&full).is_ok() {
        return Ok(full);
    }
    let fallback = context.external_config.join(path);
    if fs::symlink_metadata(&fallback).is_ok() {
        return Ok(fallback);
    }
    let parent = fallback.parent().ok_or("source has no parent")?;
    let prefix = fallback
        .file_name()
        .ok_or("source has no name")?
        .to_string_lossy();
    let mut found = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(prefix.as_ref())
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "not found: {} (looked in ~/.config)",
            path.display()
        )),
        _ => Err(format!(
            "ambiguous, matches: {}",
            found
                .iter()
                .map(|path| path.file_name().unwrap_or_default().to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

pub fn add(args: AddArgs, context: &Context) -> Result<ExitCode, String> {
    let _lock = crate::lock::MutationLock::acquire(context)?;
    let group = [
        (args.macos, "macos"),
        (args.server, "linux/server"),
        (args.hyprland, "linux/hyprland"),
        (args.kde, "linux/kde"),
        (args.arch, "linux/arch"),
        (args.ubuntu, "linux/ubuntu"),
        (args.linux, "linux/common"),
    ]
    .into_iter()
    .find_map(|(selected, group)| selected.then_some(group))
    .unwrap_or("shared");
    if let Some(package) = &args.pkg {
        validate_package(package)?;
    }
    if let Some(description) = &args.description
        && (description.is_empty() || description.contains(['\r', '\n']))
    {
        return Err("description must be a non-empty single line".into());
    }
    let source = locate_source(context, &args.path)?;
    if fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(
            if crate::fs::resolved(&source).is_ok_and(|path| {
                crate::fs::resolved(&context.root).is_ok_and(|root| path.starts_with(root))
            }) {
                format!("already managed: {}", source.display())
            } else {
                format!("refusing to adopt a foreign symlink: {}", source.display())
            },
        );
    }
    require_within(&context.home, &source)
        .map_err(|_| "source must live under $HOME".to_string())?;
    refuse_managed_children(context, &source)?;
    let (package, relative, mapping) = destination(context, &source, group, args.pkg.as_deref())?;
    validate_package(&package)?;
    let target = context.root.join(&relative);
    require_within(&context.root, &target)?;
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!("destination exists: {}", relative.display()));
    }
    let mut metadata = packages::load_metadata(&context.packages_config)?;
    if let Some(description) = args.description {
        metadata.insert(format!("{group}/{package}"), description);
    }
    let _ = config(context)?;
    let mut transaction = Transaction::new(context)?;
    transaction.move_node(&source, &target)?;
    transaction.symlink(&target, &source)?;
    if let Some(mapping) = mapping {
        append_mapping(context, &mut transaction, &mapping)?;
    }
    refresh_packages(context, &mut transaction, &metadata)?;
    transaction.stage(
        context,
        &[
            relative.clone(),
            context.targets_file.clone(),
            context.packages_config.clone(),
            context.packages_doc.clone(),
        ],
    )?;
    transaction.commit()?;
    println!(
        "moved  {} -> {}\nlinked {}",
        source.display(),
        relative.display(),
        source.display()
    );
    if let Ok(profile) = context.profile(None)
        && let Ok(groups) = crate::config::read_manifest(&context.manifest(&profile))
        && !groups.iter().any(|entry| entry == group)
    {
        println!(
            "note: group '{group}' is not in environment/{profile}/manifest, it will not be linked by 'dotfile sync' on this machine"
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn refuse_managed_children(context: &Context, source: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Ok(());
    }
    for entry in entries(source)? {
        if fs::symlink_metadata(&entry).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            if crate::fs::resolved(&entry).is_ok_and(|path| {
                crate::fs::resolved(&context.root).is_ok_and(|root| path.starts_with(root))
            }) {
                return Err(format!(
                    "already partially managed ({}), add individual files instead",
                    entry.display()
                ));
            }
        } else if entry.is_dir() {
            refuse_managed_children(context, &entry)?;
        }
    }
    Ok(())
}

fn destination(
    context: &Context,
    source: &Path,
    group: &str,
    package: Option<&str>,
) -> Result<(String, PathBuf, Option<String>), String> {
    if let Ok(relative) = source.strip_prefix(context.external_config.clone()) {
        let components = relative.components().collect::<Vec<_>>();
        let first = components
            .first()
            .ok_or("source must include a configuration name")?
            .as_os_str()
            .to_str()
            .ok_or("configuration name is not UTF-8")?;
        if components.len() > 1 {
            let package = package.unwrap_or(first).to_string();
            return Ok((
                package.clone(),
                Path::new(group)
                    .join(package)
                    .join(components[1..].iter().collect::<PathBuf>()),
                None,
            ));
        }
        if source.is_dir() {
            let package = package.unwrap_or(first).to_string();
            return Ok((package.clone(), Path::new(group).join(package), None));
        }
        let package = package
            .unwrap_or(first.split('.').next().unwrap_or(first))
            .to_string();
        return Ok((
            package.clone(),
            Path::new(group).join(&package).join(first),
            Some(format!("{group}/{package} = ~/.config")),
        ));
    }
    let package = package
        .ok_or("files outside ~/.config need --pkg <name>")?
        .to_string();
    let relative = source
        .strip_prefix(&context.home)
        .map_err(|_| "source must live under $HOME")?;
    let components = relative.components().collect::<Vec<_>>();
    let head = components
        .first()
        .ok_or("source must live under $HOME")?
        .as_os_str()
        .to_string_lossy();
    if components.len() > 1 && head.trim_start_matches('.') == package {
        Ok((
            package.clone(),
            Path::new(group)
                .join(&package)
                .join(components[1..].iter().collect::<PathBuf>()),
            Some(format!("{group}/{package} = ~/{head}")),
        ))
    } else {
        let path = Path::new(group)
            .join(&package)
            .join(source.file_name().ok_or("source has no name")?);
        let mapping = format!("{} = ~/{}", path.display(), relative.display());
        Ok((package, path, Some(mapping)))
    }
}

pub fn append_mapping(
    context: &Context,
    transaction: &mut Transaction,
    mapping: &str,
) -> Result<(), String> {
    let mut content = fs::read_to_string(&context.targets_file)
        .map_err(|error| format!("read targets: {error}"))?;
    if content.lines().any(|line| line == mapping) {
        return Ok(());
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(mapping);
    content.push('\n');
    transaction.write(&context.targets_file, content.as_bytes())
}

fn refresh_packages(
    context: &Context,
    transaction: &mut Transaction,
    metadata: &BTreeMap<String, String>,
) -> Result<(), String> {
    let groups = packages::package_groups(context)?;
    packages::validate_packages(context, &groups)?;
    let (config, document) = packages::render(context, &groups, metadata)?;
    transaction.write(&context.packages_config, config.as_bytes())?;
    transaction.write(&context.packages_doc, document.as_bytes())
}

pub fn remove(args: RemoveArgs, context: &Context) -> Result<ExitCode, String> {
    let _lock = crate::lock::MutationLock::acquire(context)?;
    let path = args.path.strip_prefix(&context.root).unwrap_or(&args.path);
    let path = path.strip_prefix("/").unwrap_or(path);
    let relative = safe_relative(path)?;
    let groups = packages::package_groups(context)?;
    packages::validate_packages(context, &groups)?;
    let group = groups
        .iter()
        .filter(|group| relative.starts_with(group) && relative != Path::new(group))
        .max_by_key(|group| group.len())
        .ok_or_else(|| format!("not a package path: {}", args.path.display()))?;
    let rest = relative
        .strip_prefix(group)
        .map_err(|error| error.to_string())?;
    let package = rest
        .components()
        .next()
        .ok_or("path must include a package")?
        .as_os_str()
        .to_str()
        .ok_or("package name is not UTF-8")?;
    validate_package(package)?;
    let package_root = context.root.join(group).join(package);
    let source = context.root.join(&relative);
    require_within(
        &context.root,
        source.parent().ok_or("source has no parent")?,
    )?;
    if fs::symlink_metadata(&source).is_err() {
        return Err(format!("not found in dotfiles: {}", relative.display()));
    }
    let configuration = config(context)?;
    validate_remove(context, &configuration, &source, &relative)?;
    let metadata = packages::load_metadata(&context.packages_config)?;
    let mut transaction = Transaction::new(context)?;
    materialize(context, &configuration, &mut transaction, &source, &relative)?;
    let mut parent = source.parent();
    while let Some(directory) = parent {
        if !directory.starts_with(&package_root) {
            break;
        }
        if directory.is_dir()
            && fs::read_dir(directory)
                .map_err(|error| error.to_string())?
                .next()
                .is_none()
        {
            transaction.remove_empty(directory)?;
        }
        parent = directory.parent();
    }
    let targets = fs::read_to_string(&context.targets_file).map_err(|error| error.to_string())?;
    let prefix = relative.to_str().ok_or("package path is not UTF-8")?;
    let kept = targets
        .lines()
        .filter(|line| {
            let Some((key, _)) = line.split_once('=') else {
                return true;
            };
            let key = key.trim();
            let key = key
                .strip_prefix("macos:")
                .or_else(|| key.strip_prefix("linux:"))
                .unwrap_or(key);
            key != prefix
                && !key
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    transaction.write(&context.targets_file, kept.as_bytes())?;
    refresh_packages(context, &mut transaction, &metadata)?;
    transaction.stage(
        context,
        &[
            relative.clone(),
            context.targets_file.clone(),
            context.packages_config.clone(),
            context.packages_doc.clone(),
        ],
    )?;
    transaction.commit()?;
    println!("removed {} from dotfiles", relative.display());
    Ok(ExitCode::SUCCESS)
}

fn mapped(
    context: &Context,
    configuration: &Configuration,
    full: &Path,
) -> Result<PathBuf, String> {
    let full = full.to_str().ok_or("package path is not UTF-8")?;
    let destination = configuration.map_destination(full).ok_or_else(|| {
        format!("no target declared for {full}; add a rule to config/targets.dotfile")
    })?;
    if !destination.is_absolute()
        || destination.parent().is_none()
        || destination.starts_with(&context.root)
        || destination
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!(
            "unsafe target for {full}: {}",
            destination.display()
        ));
    }
    Ok(destination)
}

fn validate_remove(
    context: &Context,
    configuration: &Configuration,
    source: &Path,
    full: &Path,
) -> Result<(), String> {
    mapped(context, configuration, full)?;
    if source.is_dir() && !source.is_symlink() {
        for child in entries(source)? {
            let name = child.file_name().ok_or("source has no name")?;
            validate_remove(context, configuration, &child, &full.join(name))?;
        }
    }
    Ok(())
}

fn unfold(transaction: &mut Transaction, link: &Path, source: &Path) -> Result<(), String> {
    transaction.discard(link)?;
    transaction.mkdir(link)?;
    for child in entries(source)? {
        transaction.symlink(
            &child,
            &link.join(child.file_name().ok_or("source has no name")?),
        )?;
    }
    Ok(())
}

fn materialize(
    context: &Context,
    configuration: &Configuration,
    transaction: &mut Transaction,
    source: &Path,
    full: &Path,
) -> Result<(), String> {
    let destination = mapped(context, configuration, full)?;
    let mut ancestor = PathBuf::new();
    if let Some(parent) = destination.parent() {
        for component in parent.components() {
            ancestor.push(component);
            if ancestor.is_symlink() {
                let resolved = crate::fs::resolved(&ancestor)?;
                let resolved_source = crate::fs::resolved(source)?;
                let repository = crate::fs::resolved(&context.root)?;
                if resolved == repository {
                    return Err(format!(
                        "refusing to unfold a symlink to the repository root: {}",
                        ancestor.display()
                    ));
                }
                if resolved.starts_with(&repository)
                    && resolved_source.starts_with(&resolved)
                    && resolved_source != resolved
                {
                    unfold(transaction, &ancestor, &resolved)?;
                }
            }
        }
    }
    let directory = source.is_dir() && !source.is_symlink();
    if destination.is_symlink() {
        let current = crate::fs::resolved(&destination).ok();
        let expected = crate::fs::resolved(source)?;
        if current.as_ref() != Some(&expected) {
            return transaction.discard(source);
        }
        if directory
            && (configuration.has_target_under(full.to_str().ok_or("package path is not UTF-8")?)
                || never_fold(context, &destination))
        {
            unfold(transaction, &destination, source)?;
        } else {
            transaction.discard(&destination)?;
            return transaction.move_node(source, &destination);
        }
    }
    if destination.exists() && (!directory || !destination.is_dir()) {
        return transaction.discard(source);
    }
    let existing_parent = destination
        .ancestors()
        .skip(1)
        .find(|path| path.exists() || path.is_symlink())
        .ok_or("destination has no existing ancestor")?;
    if !existing_parent.is_dir() {
        return transaction.discard(source);
    }
    if !destination.exists()
        && (!directory
            || (!configuration.has_target_under(full.to_str().ok_or("package path is not UTF-8")?)
                && !never_fold(context, &destination)))
    {
        return transaction.move_node(source, &destination);
    }
    transaction.mkdir(&destination)?;
    for child in entries(source)? {
        let name = child.file_name().ok_or("source has no name")?;
        materialize(context, configuration, transaction, &child, &full.join(name))?;
    }
    transaction.remove_empty(source)
}

pub(crate) fn entries(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort();
    Ok(entries)
}
