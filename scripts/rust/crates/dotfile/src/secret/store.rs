use super::{cli::AddArgs, recipients, sops, vault};
use crate::context::Context;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn require_vault(context: &Context) -> Result<(), String> {
    if recipients::load(context)?.is_empty() {
        return Err("no recipients enrolled (run: dotfile secret enroll <label>)".into());
    }
    sops::require_identity(context, None)?;
    Ok(())
}

pub fn add(context: &Context, args: AddArgs) -> Result<(), String> {
    if args.pkg.is_empty() {
        return Err("--pkg <name> is required".into());
    }
    if !recipients::valid_label(&args.pkg) || args.pkg == "." || args.pkg == ".." {
        return Err("--pkg needs a simple package name".into());
    }
    require_vault(context)?;
    let source = super::expand(context, &args.path);
    let meta = source
        .symlink_metadata()
        .map_err(|e| format!("not a file: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err("refusing to adopt a symlink or non-regular file".into());
    }
    let canonical = fs::canonicalize(&source).map_err(|e| e.to_string())?;
    let home = fs::canonicalize(&context.home).map_err(|e| e.to_string())?;
    if !canonical.starts_with(home) {
        return Err("source must live under $HOME".into());
    }
    let group = if args.macos {
        "macos"
    } else if args.hyprland {
        "linux/hyprland"
    } else if args.kde {
        "linux/kde"
    } else if args.arch {
        "linux/arch"
    } else if args.ubuntu {
        "linux/ubuntu"
    } else if args.linux {
        "linux/common"
    } else {
        "shared"
    };
    let package = context.root.join(group).join(&args.pkg);
    let fresh = !package.exists();
    let name = source
        .file_name()
        .ok_or("source has no filename")?
        .to_string_lossy();
    let destination = package.join(format!("{name}.enc"));
    if destination.symlink_metadata().is_ok() {
        return Err(format!("destination exists: {}", destination.display()));
    }
    let encrypted = sops::encrypt(context, &source, &destination, None)?;
    let mut changes = vec![(destination.clone(), encrypted)];
    let marker = package.join(".secret");
    if !args.no_marker && (args.marker || fresh) && !marker.exists() {
        changes.push((marker, Vec::new()));
    }
    let parent = source.parent().ok_or("source has no parent")?;
    if parent != context.home.join(".config").join(&args.pkg) {
        let target = parent
            .strip_prefix(&context.home)
            .map(|p| format!("~/{}", p.display()))
            .unwrap_or_else(|_| parent.display().to_string());
        let line = format!("{group}/{} = {target}", args.pkg);
        let mut targets = fs::read_to_string(&context.targets_file).map_err(|e| e.to_string())?;
        if !targets.lines().any(|existing| existing.trim() == line) {
            if !targets.is_empty() && !targets.ends_with('\n') {
                targets.push('\n');
            }
            targets.push_str(&line);
            targets.push('\n');
            changes.push((context.targets_file.clone(), targets.into_bytes()));
        }
    }
    let paths = changes.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
    recipients::commit(context, changes)?;
    vault::set_mode(&source, 0o600)?;
    recipients::stage(context, &paths)?;
    println!(
        "sealed {} -> {}\nkept {} (0600)",
        source.display(),
        destination.display(),
        source.display()
    );
    Ok(())
}

pub fn status(context: &Context, clean: bool, dry: bool) -> Result<ExitCode, String> {
    let configuration = super::configuration(context)?;
    let entries = vault::plan(&configuration)?;
    let variables = vault::load_variables(context);
    let mut blocked = false;
    if entries.is_empty() {
        println!("no secrets tracked");
    }
    for entry in entries {
        let result = if clean {
            vault::clean(context, &entry, &variables, dry)
        } else {
            vault::inspect(context, &entry, &variables)
        };
        match result {
            Ok(state) => {
                println!(
                    "  {}{state} {}",
                    if dry { "would " } else { "" },
                    entry.destination.display()
                );
                blocked |= matches!(state, "drifted" | "plaintext" | "unresolved" | "failed");
            }
            Err(error) => {
                println!(
                    "  {} {}: {error}",
                    if error.starts_with("unknown:") {
                        "unresolved"
                    } else {
                        "failed"
                    },
                    entry.destination.display()
                );
                blocked = true;
            }
        }
    }
    Ok(if blocked {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

pub fn vars(context: &Context, unused: bool) -> Result<ExitCode, String> {
    let variables = vault::load_variables(context);
    if !variables.ok {
        return Err(variables.note.clone());
    }
    let configuration = super::configuration(context)?;
    let mut entries = vault::plan(&configuration)?;
    for package in &configuration.packages {
        if package.kind == crate::config::PackageKind::System {
            entries.extend(vault::package_entries(&configuration, package, true)?);
        }
    }
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in entries {
        if entry.kind != vault::SecretKind::Template {
            continue;
        }
        for name in
            vault::references(&fs::read_to_string(&entry.source).map_err(|e| e.to_string())?)
        {
            used.entry(name)
                .or_default()
                .insert(entry.destination.display().to_string());
        }
    }
    if variables.values.is_empty() {
        println!("nothing declared in vars.enc.yaml");
    }
    for name in variables.values.keys() {
        if unused && used.contains_key(name) {
            continue;
        }
        println!(
            "  {name}  {}",
            used.get(name)
                .map(|paths| paths.iter().cloned().collect::<Vec<_>>().join(" "))
                .unwrap_or_else(|| "unused".into())
        );
    }
    let missing: Vec<_> = used
        .keys()
        .filter(|name| !variables.values.contains_key(*name))
        .collect();
    for name in &missing {
        println!("  {name} referenced but not defined");
    }
    Ok(if missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn edit(context: &Context, path: &Path) -> Result<ExitCode, String> {
    require_vault(context)?;
    let expanded = super::expand(context, path);
    let declared = context.root.join("vars.enc.yaml");
    let mut entries = if path == Path::new("vars")
        || path == Path::new("vars.enc.yaml")
        || expanded == declared
    {
        vec![vault::SecretEntry {
            source: declared.clone(),
            destination: std::path::PathBuf::new(),
            kind: vault::SecretKind::Encrypted,
        }]
    } else {
        vault::plan(&super::configuration(context)?)?
            .into_iter()
            .filter(|entry| {
                entry.source == expanded
                    || entry.destination == expanded
                    || entry.source.ends_with(path)
                    || entry.destination.ends_with(path)
            })
            .collect()
    };
    if entries.len() != 1 {
        return Err(if entries.is_empty() {
            format!("no tracked secret matches '{}'", path.display())
        } else {
            format!("'{}' matches several secrets", path.display())
        });
    }
    let mut entry = entries.remove(0);
    let fresh = !entry.source.exists();
    let temporary = tempfile::Builder::new()
        .prefix("dotfile-secret-edit-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let working = temporary
        .path()
        .join(entry.source.file_name().ok_or("secret has no filename")?);
    if fresh {
        if entry.source != declared {
            return Err("nothing to edit".into());
        }
        let seed = temporary.path().join("seed.yaml");
        vault::write_private(&seed, b"{}\n")?;
        let encrypted = sops::encrypt(context, &seed, &entry.source, None)?;
        vault::write_private(&working, &encrypted)?;
    } else {
        vault::write_private(
            &working,
            &vault::read_source(&entry.source, sops::MAX_SECRET_BYTES * 4)?,
        )?;
    }
    let mut command = sops::command(context, None);
    command
        .arg("--config")
        .arg(context.root.join(".sops.yaml"))
        .arg(&working)
        .env("TMPDIR", temporary.path())
        .env("TMP", temporary.path())
        .env("TEMP", temporary.path())
        .stdin(std::process::Stdio::inherit());
    let status = crate::process::status(&mut command)
        .map_err(|e| format!("SOPS editor could not start: {e}"))?;
    if !status.success() && status.code() != Some(200) {
        return Ok(ExitCode::from(status.code().unwrap_or(1) as u8));
    }
    crate::fs::write_generated(
        &entry.source,
        &vault::read_source(&working, sops::MAX_SECRET_BYTES * 4)?,
    )?;
    recipients::stage(context, &[entry.source.clone()])?;
    if status.code() == Some(200) {
        println!("unchanged {}", entry.source.display());
        return Ok(ExitCode::SUCCESS);
    }
    if entry.destination.as_os_str().is_empty() {
        println!("saved {}", entry.source.display());
        return Ok(ExitCode::SUCCESS);
    }
    let variables = vault::load_variables(context);
    let result = vault::materialize(context, &mut entry, &variables, false, true)?;
    println!("{} {}", result.detail, entry.destination.display());
    Ok(if result.blocked {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
