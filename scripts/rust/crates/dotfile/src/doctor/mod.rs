//! Read-only workstation diagnostics. Independent probes run concurrently;
//! rendering always follows the same section order.
use crate::config::{Configuration, blocks, profiles};
use crate::context::Context;
use crate::event::{Event, VecSink};
use clap::Args as ClapArgs;
use report::{Manager, Package, Row, Status};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;
use workstation::path::home_relative_in;

pub mod report;

#[derive(Debug, ClapArgs)]
pub struct Args {
    pub profile: Option<String>,
    /// Include optional packages and list every detail
    #[arg(long = "all")]
    pub show_all: bool,
}

struct Probes<'a> {
    context: &'a Context,
    commands: Mutex<BTreeMap<String, Option<PathBuf>>>,
    outputs: Mutex<BTreeMap<Vec<String>, Result<String, String>>>,
}
impl<'a> Probes<'a> {
    fn new(context: &'a Context) -> Self {
        Self {
            context,
            commands: Mutex::new(BTreeMap::new()),
            outputs: Mutex::new(BTreeMap::new()),
        }
    }
    fn path(&self, name: &str) -> Option<PathBuf> {
        let mut commands = self
            .commands
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        commands
            .entry(name.into())
            .or_insert_with(|| profiles::command_path(self.context, name))
            .clone()
    }
    fn output(&self, words: &[&str]) -> Result<String, String> {
        let key = words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>();
        if let Some(output) = self
            .outputs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key)
        {
            return output.clone();
        }
        let program = self
            .path(words[0])
            .ok_or_else(|| format!("{} is not installed", words[0]))?;
        let mut command = self.context.command(program);
        command.args(&words[1..]).stdin(std::process::Stdio::null());
        let result = crate::process::output(
            &mut command,
            hostkit::process::CaptureLimits {
                stdout: 4 * 1024 * 1024,
                stderr: 16 * 1024,
            },
            Duration::from_secs(10),
        )
        .map_err(|error| format!("{}: {error}", words[0]))
        .and_then(|output| {
            if !output.status.success() {
                Err(format!("{} failed ({})", words[0], output.status))
            } else if output.stdout_truncated {
                Err(format!("{} output exceeded the capture limit", words[0]))
            } else {
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
        });
        self.outputs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(key, result.clone());
        result
    }
}

pub fn run(args: Args, context: &Context) -> Result<ExitCode, String> {
    let profile = context.profile(args.profile.as_deref())?;
    let configuration = Configuration::load(context, &profile, &[], &VecSink::default())?;
    let groups = crate::config::read_manifest(&context.manifest(&profile))?;
    let probes = Probes::new(context);
    let platform = profiles::platform();
    let manager = default_manager(&profile, &platform, &probes);
    let rows = std::thread::scope(|scope| -> Result<Vec<Row>, String> {
        let requirements = scope.spawn(|| requirement_rows(context, &groups, manager, &probes));
        let pins = scope.spawn(|| pin_rows(context, &groups, &probes));
        let other = scope.spawn(|| -> Result<Vec<Row>, String> {
            let mut rows = plugin_rows(context, &groups, &probes)?;
            rows.extend(package_rows(context, &profile, &groups, &probes)?);
            rows.extend(benchmark_rows(context, &probes)?);
            Ok(rows)
        });
        let mut rows = link_rows(context, &configuration)?;
        rows.extend(environment_rows(
            context,
            &profile,
            &platform,
            &groups,
            &configuration,
            &probes,
        )?);
        rows.extend(
            requirements
                .join()
                .map_err(|_| "requirements probe panicked")??,
        );
        rows.extend(pins.join().map_err(|_| "version probe panicked")??);
        rows.extend(other.join().map_err(|_| "package probe panicked")??);
        Ok(rows)
    })?;
    print!(
        "{}",
        report::render(
            &report::Report {
                profile: &profile,
                rows: &rows,
                show_all: args.show_all,
            },
            &workstation::Style::for_stdout(),
        )
    );
    Ok(if rows.iter().any(|row| row.problems > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The profile names its platform first; the host platform and PATH are fallbacks.
fn default_manager(profile: &str, platform: &str, probes: &Probes<'_>) -> Option<Manager> {
    Manager::for_platform(profile.split('/').next().unwrap_or_default())
        .or_else(|| Manager::for_platform(platform))
        .or_else(|| {
            [Manager::Brew, Manager::Pacman, Manager::Apt]
                .into_iter()
                .find(|manager| probes.path(manager.program()).is_some())
        })
}

fn link_rows(context: &Context, configuration: &Configuration) -> Result<Vec<Row>, String> {
    let (entries, merge_paths) = crate::sync::merge::discover(context, configuration)?;
    let events = VecSink::default();
    let (decisions, _server) = crate::decision::channel_for(false);
    let links = crate::sync::links::synchronize(
        context,
        configuration,
        &merge_paths,
        true,
        &decisions,
        &events,
    );
    let (client, _server) = crate::decision::channel_for(false);
    let merges = crate::sync::merge::synchronize(
        context,
        &entries,
        true,
        false,
        crate::cli::Resolution::Skip,
        &client,
        &events,
    )?;
    let mut details = Vec::new();
    let mut notes = Vec::new();
    let mut checked = 0_usize;
    let mut missing = 0_usize;
    for event in events.events() {
        if let Event::Item {
            path,
            detail,
            changed,
            ..
        } = event
        {
            checked += 1;
            if changed
                || detail.contains("conflict")
                || detail.contains("edited")
                || detail.contains("blocked")
                || detail.contains("drifted")
                || detail.contains("not valid JSON")
            {
                if fs::symlink_metadata(&path).is_err() {
                    missing += 1;
                }
                details.push((home_relative_in(&path, &context.home), detail));
            } else if detail.contains("formatting") {
                notes.push((home_relative_in(&path, &context.home), detail));
            }
        }
    }
    let problems = details
        .len()
        .max(merges.blocked)
        .max(usize::from(links.is_err()));
    let summary = [
        (checked.saturating_sub(problems), "linked"),
        (missing, "missing"),
        (problems.saturating_sub(missing), "differing"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, word)| format!("{count} {word}"))
    .collect::<Vec<_>>()
    .join(", ");
    let mut row = Row::new(
        if problems > 0 {
            Status::Bad
        } else {
            Status::Ok
        },
        "links",
        if summary.is_empty() {
            "nothing linked".into()
        } else {
            summary
        },
        problems,
    );
    row.details = details;
    row.details.extend(notes);
    if let Err(error) = links {
        row.details.push((error, String::new()));
    }
    Ok(vec![row])
}

fn environment_rows(
    context: &Context,
    profile: &str,
    platform: &str,
    groups: &[String],
    configuration: &Configuration,
    probes: &Probes<'_>,
) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    if !platform.is_empty()
        && context.environment_dir.join(platform).is_dir()
        && profile.split('/').next() != Some(platform)
    {
        rows.push(Row::new(
            Status::Warn,
            "profile",
            format!("not a {platform} profile (host {platform})"),
            1,
        ));
    }
    let project = context.root.join("scripts/python/pyproject.toml");
    if project.is_file() {
        let text = fs::read_to_string(&project).map_err(|error| error.to_string())?;
        let value: toml::Value =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", project.display()))?;
        let commands = value
            .get("project")
            .and_then(|value| value.get("scripts"))
            .and_then(toml::Value::as_table)
            .map(|scripts| scripts.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let bin = context.home.join("dotfiles/.bin");
        let path_has_bin = std::env::split_paths(&context.env("PATH").unwrap_or_default())
            .any(|path| crate::fs::resolved(&path).ok() == crate::fs::resolved(&bin).ok());
        let mut details = Vec::new();
        if !path_has_bin {
            details.push(("~/dotfiles/.bin is not on PATH".into(), String::new()));
        }
        let data = context
            .env("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| context.home.join(".local/share"));
        // setup.sh installs into ~/dotfiles/.uv; an explicit UV_TOOL_DIR is honored too.
        let uv_dirs = [
            context.home.join("dotfiles/.uv"),
            context
                .env("UV_TOOL_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data.join("uv/tools")),
        ]
        .map(|dir| crate::fs::resolved(&dir).unwrap_or(dir));
        for name in commands {
            let installed = bin.join(&name);
            if !installed.is_file() {
                details.push((name, "missing from ~/dotfiles/.bin".into()));
            } else if !crate::fs::resolved(&installed)
                .is_ok_and(|path| uv_dirs.iter().any(|dir| path.starts_with(dir)))
            {
                details.push((name, "not installed by uv".into()));
            } else if probes
                .path(&name)
                .and_then(|path| fs::canonicalize(path).ok())
                != fs::canonicalize(&installed).ok()
            {
                details.push((name, "shadowed on PATH".into()));
            }
        }
        if !details.is_empty() {
            let summary = match details
                .iter()
                .map(|(_, hint)| hint.as_str())
                .collect::<BTreeSet<_>>()
            {
                reasons if reasons.len() == 1 => {
                    format!(
                        "{} {}",
                        details.len(),
                        reasons.into_iter().next().unwrap_or_default()
                    )
                }
                _ => format!("{} need attention", details.len()),
            };
            let mut row = Row::new(Status::Warn, "commands", summary, 1);
            row.details = details;
            rows.push(row);
        }
    }
    let pending = groups
        .iter()
        .filter(|group| {
            context.root.join(group).join("overrides").is_dir()
                && !configuration.overrides.contains_key(*group)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !pending.is_empty() {
        rows.push(Row::new(
            Status::Warn,
            "overrides",
            format!("unselected for {}", pending.join(" ")),
            1,
        ));
    }
    let shell = login_shell(context, probes);
    if !shell.is_empty()
        && Path::new(&shell)
            .file_name()
            .is_some_and(|name| name != "zsh")
    {
        rows.push(Row::new(
            Status::Warn,
            "shell",
            format!("login shell is {shell}, not zsh"),
            1,
        ));
    }
    Ok(rows)
}

fn login_shell(context: &Context, probes: &Probes<'_>) -> String {
    let user = context
        .env("USER")
        .map(|user| user.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !user.is_empty() {
        if let Ok(output) = probes.output(&["getent", "passwd", &user])
            && let Some(shell) = output.trim().rsplit(':').next()
        {
            return shell.into();
        }
        if std::env::consts::OS == "macos"
            && let Ok(output) =
                probes.output(&["dscl", ".", "-read", &format!("/Users/{user}"), "UserShell"])
            && let Some((_, shell)) = output.split_once(':')
        {
            return shell.trim().into();
        }
    }
    context
        .env("SHELL")
        .map(|shell| shell.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn grouped(context: &Context, file: &str, groups: &[String]) -> Result<Vec<blocks::Entry>, String> {
    let path = context.root_config.join(file);
    let entries = blocks::read(&path)?;
    for entry in &entries {
        if entry.opens && !context.root.join(&entry.block).is_dir() {
            return Err(format!(
                "{}:{}: unknown group: {}",
                path.display(),
                entry.number,
                entry.block
            ));
        }
    }
    Ok(entries
        .into_iter()
        .filter(|entry| !entry.opens && groups.contains(&entry.block))
        .collect())
}

struct Requirement {
    package: String,
    manager: Option<Manager>,
    optional: bool,
}

fn requirement_rows(
    context: &Context,
    groups: &[String],
    default_manager: Option<Manager>,
    probes: &Probes<'_>,
) -> Result<Vec<Row>, String> {
    let mut requirements = BTreeMap::<(String, String), Requirement>::new();
    for entry in grouped(context, "requirements.dotfile", groups)? {
        let (name, value) = entry.split();
        let optional = name.starts_with('?');
        let name = name.trim_start_matches('?').trim();
        let (kind, name) = if let Some(name) = name.strip_prefix("font ") {
            ("font", name.trim())
        } else if let Some(name) = name.strip_prefix("file ") {
            ("file", name.trim())
        } else {
            ("command", name)
        };
        if name.is_empty() {
            return Err(format!(
                "config/requirements.dotfile:{}: empty entry",
                entry.number
            ));
        }
        let (manager, package) = match value.split_once(':') {
            Some((tag, package)) if Manager::parse(tag).is_some() => {
                (Manager::parse(tag), package.trim())
            }
            _ => (default_manager, value),
        };
        let package = if package.is_empty() && kind == "command" {
            name
        } else {
            package
        };
        // Later blocks refine earlier ones: platform groups rename shared packages.
        let previous = requirements.remove(&(kind.to_string(), name.to_string()));
        requirements.insert(
            (kind.to_string(), name.to_string()),
            Requirement {
                package: package.to_string(),
                manager,
                optional: optional && previous.is_none_or(|previous| previous.optional),
            },
        );
    }
    let fonts = if requirements.keys().any(|(kind, _)| kind == "font") {
        installed_fonts(context, probes)?
    } else {
        BTreeSet::new()
    };
    let mut rows = Vec::new();
    let mut optional = Row::new(Status::Note, "optional", String::new(), 0);
    for (kind, label) in [("command", "tools"), ("font", "fonts"), ("file", "files")] {
        let mut wanted = 0;
        let mut packages = Vec::new();
        let mut details = Vec::new();
        for ((entry_kind, name), requirement) in &requirements {
            if entry_kind != kind {
                continue;
            }
            let gone = match kind {
                "command" => probes.path(name).is_none(),
                "file" => !crate::manage::expand(context, Path::new(name))?.exists(),
                _ => font_missing(name, &fonts),
            };
            if !requirement.optional {
                wanted += 1;
            }
            if !gone {
                continue;
            }
            if requirement.package.is_empty() {
                let detail = (name.clone(), String::new());
                if requirement.optional {
                    optional.details.push(detail);
                } else {
                    details.push(detail);
                }
                continue;
            }
            let package = Package {
                manager: requirement.manager,
                name: requirement.package.clone(),
                optional: requirement.optional,
            };
            if requirement.optional {
                optional.packages.push(package);
            } else {
                packages.push(package);
            }
        }
        if wanted == 0 {
            continue;
        }
        let mut row = Row::missing(label, wanted, packages);
        if !details.is_empty() {
            row.status = Status::Bad;
            row.problems += details.len();
            row.summary = format!("{} of {wanted} missing", row.problems);
            row.details = details;
        }
        rows.push(row);
    }
    let absent = optional.packages.len() + optional.details.len();
    if absent > 0 {
        optional.summary = format!("{absent} not installed");
        rows.push(optional);
    }
    Ok(rows)
}

fn pin_rows(context: &Context, groups: &[String], probes: &Probes<'_>) -> Result<Vec<Row>, String> {
    let mut pins = BTreeSet::new();
    for entry in grouped(context, "pins.dotfile", groups)? {
        let (name, wanted) = entry.split();
        if name.is_empty() || wanted.is_empty() {
            return Err(format!(
                "config/pins.dotfile:{}: expected command = build",
                entry.number
            ));
        }
        pins.insert((name.to_string(), wanted.to_string()));
    }
    if pins.is_empty() {
        return Ok(Vec::new());
    }
    let mut wrong = Vec::new();
    for (name, wanted) in &pins {
        match probes.output(&[name, "--version"]) {
            Ok(output) if output.lines().next().unwrap_or_default().contains(wanted) => {}
            Ok(output) => wrong.push((
                name.clone(),
                format!(
                    "{}, want {wanted}",
                    output.lines().next().unwrap_or("no version output")
                ),
            )),
            Err(error) => wrong.push((name.clone(), format!("{error}, want {wanted}"))),
        }
    }
    let mut row = Row::new(
        if wrong.is_empty() {
            Status::Ok
        } else {
            Status::Bad
        },
        "pins",
        if wrong.is_empty() {
            format!("{} pinned", pins.len())
        } else {
            format!("{} mismatched", wrong.len())
        },
        wrong.len(),
    );
    row.details = wrong;
    Ok(vec![row])
}

fn font_key(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}
fn font_missing(name: &str, fonts: &BTreeSet<String>) -> bool {
    let name = font_key(name);
    !fonts.iter().any(|font| {
        font.strip_prefix(&name).is_some_and(|suffix| {
            let suffix = suffix
                .strip_suffix("italic")
                .or_else(|| suffix.strip_suffix("oblique"))
                .unwrap_or(suffix);
            [
                "",
                "regular",
                "normal",
                "book",
                "text",
                "thin",
                "extralight",
                "ultralight",
                "light",
                "medium",
                "retina",
                "semibold",
                "demibold",
                "bold",
                "extrabold",
                "ultrabold",
                "black",
                "heavy",
            ]
            .contains(&suffix)
        })
    })
}
fn installed_fonts(context: &Context, probes: &Probes<'_>) -> Result<BTreeSet<String>, String> {
    if let Ok(output) = probes.output(&["fc-list", "--format", "%{family}\n"])
        && !output.trim().is_empty()
    {
        return Ok(output
            .lines()
            .flat_map(|line| line.split(','))
            .map(font_key)
            .collect());
    }
    let mut fonts = BTreeSet::new();
    for directory in [
        "~/Library/Fonts",
        "/Library/Fonts",
        "/System/Library/Fonts",
        "~/.local/share/fonts",
        "~/.fonts",
        "/usr/share/fonts",
        "/usr/local/share/fonts",
    ] {
        let path = crate::manage::expand(context, Path::new(directory))?;
        walk_files(&path, &mut |path| {
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    ["ttf", "otf", "ttc", "dfont", "pfb"]
                        .contains(&extension.to_lowercase().as_str())
                })
            {
                fonts.insert(font_key(
                    &path.file_stem().unwrap_or_default().to_string_lossy(),
                ));
            }
        })?;
    }
    Ok(fonts)
}

fn plugin_rows(
    context: &Context,
    groups: &[String],
    probes: &Probes<'_>,
) -> Result<Vec<Row>, String> {
    let mut plugins = BTreeSet::new();
    for group in groups {
        walk_files(&context.root.join(group).join("zsh"), &mut |path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.ends_with(".zsh") && !name.ends_with("zshrc") {
                return;
            }
            if let Ok(text) = fs::read_to_string(path) {
                for tail in text.split("$ZSH/custom/plugins/").skip(1) {
                    let name = tail
                        .chars()
                        .take_while(|character| {
                            character.is_ascii_alphanumeric() || "._-".contains(*character)
                        })
                        .collect::<String>();
                    if !name.is_empty() {
                        plugins.insert(name);
                    }
                }
                for word in text.split(|character: char| {
                    character.is_whitespace() || ['\'', '"', '$', ')', '('].contains(&character)
                }) {
                    let path = Path::new(word);
                    if let (Some(parent), Some(filename)) =
                        (path.parent().and_then(Path::file_name), path.file_name())
                    {
                        let parent = parent.to_string_lossy();
                        let filename = filename.to_string_lossy();
                        if filename == format!("{parent}.zsh")
                            || filename == format!("{parent}.plugin.zsh")
                        {
                            plugins.insert(parent.into_owned());
                        }
                    }
                }
            }
        })?;
    }
    if plugins.is_empty() {
        return Ok(Vec::new());
    }
    let zsh = context
        .env("ZSH")
        .map(PathBuf::from)
        .unwrap_or_else(|| context.home.join(".oh-my-zsh"));
    if !zsh.is_dir() {
        return Ok(vec![Row::new(
            Status::Bad,
            "oh-my-zsh",
            format!("not installed at {}", home_relative_in(&zsh, &context.home)),
            1,
        )]);
    }
    let brew = context
        .env("HOMEBREW_PREFIX")
        .map(PathBuf::from)
        .or_else(|| {
            probes
                .path("brew")
                .and_then(|path| path.parent()?.parent().map(Path::to_path_buf))
        });
    let missing: Vec<(String, String)> = plugins
        .iter()
        .filter(|name| {
            !zsh.join("custom/plugins").join(name).is_dir()
                && !zsh.join("plugins").join(name).is_dir()
                && !["/usr/share/zsh/plugins", "/usr/share", "/usr/local/share"]
                    .iter()
                    .any(|parent| Path::new(parent).join(name).is_dir())
                && !brew
                    .as_ref()
                    .is_some_and(|prefix| prefix.join("share").join(name).is_dir())
        })
        .map(|name| (name.clone(), String::new()))
        .collect();
    if missing.is_empty() {
        return Ok(vec![Row::new(
            Status::Ok,
            "plugins",
            format!("{} installed", plugins.len()),
            0,
        )]);
    }
    let mut row = Row::new(
        Status::Bad,
        "plugins",
        format!("{} of {} missing", missing.len(), plugins.len()),
        missing.len(),
    );
    row.details = missing;
    Ok(vec![row])
}

fn package_rows(
    context: &Context,
    profile: &str,
    groups: &[String],
    probes: &Probes<'_>,
) -> Result<Vec<Row>, String> {
    let mut sources = Vec::new();
    let brewfile = context.root.join("macos/Brewfile");
    if groups.iter().any(|group| group == "macos") && brewfile.is_file() {
        let mut names = Vec::new();
        for line in fs::read_to_string(brewfile)
            .map_err(|error| error.to_string())?
            .lines()
        {
            let line = line.split('#').next().unwrap_or_default().trim();
            if (line.starts_with("brew ") || line.starts_with("cask "))
                && let Some(start) = line.find(['\'', '"'])
            {
                let quote = line.as_bytes()[start] as char;
                if let Some(end) = line[start + 1..].find(quote) {
                    names.push(
                        line[start + 1..start + 1 + end]
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_string(),
                    );
                }
            }
        }
        sources.push(("brewfile", Manager::Brew, names));
    }
    for (file, label, manager) in [
        ("pkglist.txt", "pkglist", Manager::Pacman),
        ("aurlist.txt", "aurlist", Manager::Aur),
    ] {
        let path = context.environment_dir.join(profile).join(file);
        if path.is_file() {
            sources.push((label, manager, crate::config::read_manifest(&path)?));
        }
    }
    let mut rows = Vec::new();
    let mut cached = BTreeMap::<&str, Result<BTreeSet<String>, String>>::new();
    let mut skipped = BTreeSet::new();
    for (label, manager, wanted) in sources {
        // AUR packages show up in pacman's database like any other.
        let inventory = if manager == Manager::Brew {
            "brew"
        } else {
            "pacman"
        };
        if probes.path(inventory).is_none() {
            if skipped.insert(inventory) {
                rows.push(Row::new(
                    Status::Note,
                    inventory,
                    "not installed, package lists skipped",
                    0,
                ));
            }
            continue;
        }
        let installed = cached.entry(inventory).or_insert_with(|| {
            if inventory == "brew" {
                let formula = probes.output(&["brew", "list", "--formula", "-1"])?;
                let cask = probes.output(&["brew", "list", "--cask", "-1"])?;
                Ok(formula
                    .split_whitespace()
                    .chain(cask.split_whitespace())
                    .map(str::to_string)
                    .collect())
            } else {
                Ok(probes
                    .output(&["pacman", "-Qq"])?
                    .split_whitespace()
                    .map(str::to_string)
                    .collect())
            }
        });
        match installed {
            Ok(installed) => rows.push(Row::missing(
                label,
                wanted.len(),
                wanted
                    .into_iter()
                    .filter(|name| {
                        !inventory_has(installed, name, manager) && probes.path(name).is_none()
                    })
                    .map(|name| Package {
                        manager: Some(manager),
                        name,
                        optional: false,
                    })
                    .collect(),
            )),
            Err(error) => {
                if skipped.insert(inventory) {
                    rows.push(Row::new(Status::Bad, inventory, error.clone(), 1));
                }
            }
        }
    }
    Ok(rows)
}

/// Brew lists versioned formulae like `python@3.14` for a Brewfile entry `python`.
fn inventory_has(installed: &BTreeSet<String>, name: &str, manager: Manager) -> bool {
    installed.contains(name)
        || manager == Manager::Brew
            && installed.iter().any(|package| {
                package
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('@'))
            })
}

fn benchmark_rows(context: &Context, probes: &Probes<'_>) -> Result<Vec<Row>, String> {
    let mut inventory = context.inventory();
    inventory.host = context
        .env("HWTUNE_HOST")
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty());
    inventory.config = None;
    let mut host = sysinfo::inventory::resolve_with(&inventory, &[], "", &[]);
    if host.is_empty() {
        let hosts = match sysinfo::inventory::load_hosts_from(&inventory.hosts_file()) {
            Ok(hosts) => hosts,
            Err(error) => return Ok(vec![Row::new(Status::Bad, "benchmark", error, 1)]),
        };
        let hostname = context
            .env("HOSTNAME")
            .map(|value| value.to_string_lossy().trim().to_string())
            .filter(|value| !value.is_empty());
        let local = sysinfo::inventory::local_hostnames_with(hostname.as_deref(), |words| {
            probes.output(words).ok()
        });
        host = sysinfo::inventory::resolve_with(&inventory, &hosts, "", &local);
    }
    if host.is_empty() {
        return Ok(Vec::new());
    }
    if !sysinfo::inventory::valid_name(&host) {
        return Err("benchmark host must be a single path component".into());
    }
    let store = hwtune::bench::store::Store::new(
        context
            .env("HWTUNE_BENCHMARKS")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| context.root.join("benchmarks")),
    );
    let runs = store.list_runs(Some(&host), hwtune::bench::record::CLEAN)?;
    if runs.is_empty() {
        return Ok(vec![Row::new(
            Status::Note,
            "benchmark",
            format!("no runs recorded for {host}"),
            0,
        )]);
    }
    let issues = hwtune::bench::health::issues_for_runs(&store, &host, &runs)?;
    if issues.is_empty() {
        return Ok(vec![Row::new(
            Status::Ok,
            "benchmark",
            format!("{} clean runs; newest {}", runs.len(), runs[0].started),
            0,
        )]);
    }
    Ok(issues
        .into_iter()
        .map(|issue| {
            let mut row = Row::new(
                if issue.severity == sysinfo::model::Severity::Error {
                    Status::Bad
                } else {
                    Status::Warn
                },
                "benchmark",
                issue.title,
                1,
            );
            row.details.push((issue.detail, issue.action));
            row
        })
        .collect())
}

fn walk_files(directory: &Path, visit: &mut impl FnMut(&Path)) -> Result<(), String> {
    if !directory.is_dir() {
        return Ok(());
    }
    for path in crate::manage::entries(directory)? {
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk_files(&path, visit)?;
        } else if metadata.is_file() {
            visit(&path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/doctor_tests.rs"]
mod tests;
