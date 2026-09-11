use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::context::Context;
use clap::CommandFactory;
use serde::Deserialize;

pub use workstation::surface::{Command, Completion, Param};

pub fn native() -> Command {
    let mut command = crate::cli::Cli::command();
    command.build();
    let mut tree = workstation::surface::from_clap(&command, vec!["dotfile".into()]);
    customize(&mut tree);
    tree.children.push(Command {
        path: vec!["dotfile".into(), "format".into()],
        help: "Format configured files".into(),
        delegate: Some("dotfile-format".into()),
        ..Default::default()
    });
    tree
}

fn customize(command: &mut Command) {
    let label = command.label();
    for parameter in &mut command.params {
        let key = if parameter.kind == "option" {
            parameter.flag()
        } else {
            &parameter.name
        };
        let source = match key {
            "profile" if label.starts_with("dotfile theme") => Some("theme-profiles"),
            "profile" => Some("profiles"),
            "scope" if label.starts_with("dotfile theme") => Some("theme-scopes"),
            "--to" => Some("hosts"),
            "--pkg" if label.starts_with("dotfile dev") => Some("dev-packages"),
            "--lang" if label.starts_with("dotfile dev") => Some("dev-languages"),
            "--pkg" => Some("packages"),
            "label" if label.starts_with("dotfile secret") => Some("recipients"),
            "path" if label == "dotfile remove" => Some("tracked"),
            "path" if label == "dotfile secret edit" => Some("secrets"),
            "path" if label == "dotfile system diff" => Some("system-files"),
            _ => None,
        };
        if let Some(source) = source {
            parameter.completion = Some(Completion::Call {
                source: source.into(),
            });
        } else if key == "--override" {
            parameter.completion = Some(Completion::Pair {
                groups: "override-groups".into(),
                names: "override-names".into(),
            });
        } else if key == "--using" {
            parameter.completion = Some(Completion::Files {
                pattern: "*.txt".into(),
            });
        } else if key == "--group" {
            parameter.choices = crate::artifacts::packages::DEFAULT_GROUPS
                .iter()
                .map(|group| group.to_string())
                .collect();
        } else if matches!(key, "path" | "paths") && parameter.completion.is_none() {
            parameter.completion = Some(Completion::Files {
                pattern: String::new(),
            });
        }
    }
    for child in &mut command.children {
        customize(child);
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredSurface {
    pub version: u32,
    pub commands: BTreeMap<String, Command>,
}

pub fn declared(context: &Context) -> Result<DeclaredSurface, String> {
    let path = context.root.join("config/command-surface.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DeclaredSurface::default()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let surface: DeclaredSurface =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if surface.version != 2 {
        return Err("unsupported command metadata version; expected 2".into());
    }
    fn identifier(value: &str) -> bool {
        !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
    }
    fn validate(command: &Command, expected: &[String]) -> Result<(), String> {
        if command.path != expected || !command.path.iter().all(|part| identifier(part)) {
            return Err(format!("invalid command path: {}", command.label()));
        }
        if !command.aliases.iter().all(|alias| identifier(alias))
            || command
                .delegate
                .as_ref()
                .is_some_and(|delegate| !identifier(delegate))
        {
            return Err(format!(
                "invalid command alias or delegate: {}",
                command.label()
            ));
        }
        for parameter in &command.params {
            if let Some(Completion::Call { source }) = &parameter.completion
                && !identifier(source)
            {
                return Err(format!("invalid completion source: {source}"));
            }
            if let Some(Completion::Pair { groups, names }) = &parameter.completion
                && (!identifier(groups) || !identifier(names))
            {
                return Err(format!(
                    "invalid paired completion source: {}",
                    parameter.name
                ));
            }
        }
        let mut children = std::collections::BTreeSet::new();
        for child in &command.children {
            if !children.insert(child.name()) {
                return Err(format!("duplicate command: {}", child.label()));
            }
            let mut path = expected.to_vec();
            path.push(child.name().into());
            validate(child, &path)?;
        }
        Ok(())
    }
    for (name, command) in &surface.commands {
        validate(command, std::slice::from_ref(name))?;
        if name == "dotfile" {
            return Err("declarative metadata cannot override dotfile".into());
        }
    }
    Ok(surface)
}

fn resolver(context: &Context) -> workstation::native::Resolver {
    workstation::native::Resolver {
        root: context.root.clone(),
        home: context.home.clone(),
        current_exe: std::env::current_exe().ok().filter(|path| {
            path.starts_with(&context.root) || path.starts_with(context.home.join(".local/bin"))
        }),
        manifest: context.env("DOTFILE_DEV_BUILD_MANIFEST").map(PathBuf::from),
    }
}

pub fn binaries(
    context: &Context,
    programs: &[String],
) -> Result<BTreeMap<String, PathBuf>, String> {
    resolver(context).resolve_many(programs)
}

pub fn binary(context: &Context, program: &str) -> Result<Option<PathBuf>, String> {
    resolver(context).resolve(program)
}

pub fn external(context: &Context, program: &str) -> Result<Option<Command>, String> {
    let Some(binary) = binary(context, program)? else {
        return Ok(None);
    };
    external_at(context, program, &binary)
}

pub fn external_many(
    context: &Context,
    programs: &[String],
) -> Result<BTreeMap<String, Command>, String> {
    let binaries = binaries(context, programs)?.into_iter().collect::<Vec<_>>();
    if binaries.is_empty() {
        return Ok(BTreeMap::new());
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    let chunk_size = binaries.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles = binaries
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(name, binary)| {
                            external_at(context, name, binary).map(|tree| (name.clone(), tree))
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
            })
            .collect::<Vec<_>>();
        let mut trees = BTreeMap::new();
        for handle in handles {
            for (name, tree) in handle
                .join()
                .map_err(|_| "command metadata worker failed".to_string())??
            {
                if let Some(tree) = tree {
                    trees.insert(name, tree);
                }
            }
        }
        Ok(trees)
    })
}

fn external_at(
    context: &Context,
    program: &str,
    binary: &std::path::Path,
) -> Result<Option<Command>, String> {
    let output = crate::process::output(
        context.command(binary).arg("--command-dump"),
        hostkit::process::CaptureLimits::default(),
        Duration::from_secs(5),
    )
    .map_err(|e| format!("{program}: {e}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    if output.stdout_truncated {
        return Err(format!("{program}: command metadata exceeds limit"));
    }
    parse_dump(&String::from_utf8_lossy(&output.stdout), program).map(Some)
}

pub fn parse_dump(text: &str, program: &str) -> Result<Command, String> {
    if text.trim_start().starts_with('{') {
        let mut document: workstation::surface::Document =
            serde_json::from_str(text).map_err(|error| format!("{program}: {error}"))?;
        if document.version != workstation::surface::VERSION {
            return Err(format!(
                "{program}: unsupported command schema version {}",
                document.version
            ));
        }
        fn rename(command: &mut Command, program: &str) {
            if let Some(root) = command.path.first_mut() {
                *root = program.into();
            }
            for child in &mut command.children {
                rename(child, program);
            }
        }
        rename(&mut document.command, program);
        return Ok(document.command);
    }
    // Installed binaries may predate the JSON protocol until the next setup.

    let mut commands: Vec<Command> = Vec::new();
    for line in text.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.first() == Some(&"C") && fields.len() >= 4 {
            let mut path = fields[1].split(' ').map(str::to_string).collect::<Vec<_>>();
            if let Some(first) = path.first_mut() {
                *first = program.into();
            }
            commands.push(Command {
                path,
                hidden: fields[2] == "1",
                help: fields[3].into(),
                ..Default::default()
            });
        } else if fields.first() == Some(&"A")
            && fields.len() >= 10
            && let Some(command) = commands.last_mut()
        {
            command.params.push(Param {
                kind: fields[2].into(),
                name: fields[3].into(),
                opts: fields[4]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                metavar: fields[5].into(),
                multiple: fields[6] == "1",
                required: fields[7] == "1",
                hidden: fields[8] == "1",
                help: fields[9].into(),
                ..Default::default()
            });
        }
    }
    if commands.is_empty() {
        return Err(format!("{program}: invalid command metadata"));
    }
    fn build(index: usize, commands: &[Command]) -> Command {
        let mut command = commands[index].clone();
        command.children = commands
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.path.len() == command.path.len() + 1 && c.path.starts_with(&command.path)
            })
            .map(|(i, _)| build(i, commands))
            .collect();
        command
    }
    Ok(build(0, &commands))
}
