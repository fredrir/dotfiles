use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::context::Context;
use clap::CommandFactory;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Param {
    pub kind: String,
    pub name: String,
    pub opts: Vec<String>,
    #[serde(default)]
    pub secondary: Vec<String>,
    pub metavar: String,
    pub help: String,
    pub multiple: bool,
    pub required: bool,
    pub hidden: bool,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
}
impl Param {
    pub fn flag(&self) -> &str {
        self.opts
            .iter()
            .find(|s| s.starts_with("--"))
            .or_else(|| self.opts.first())
            .map_or(&self.name, String::as_str)
    }
    pub fn standard(&self) -> bool {
        matches!(self.flag(), "--help" | "--completions" | "--version")
    }
    pub fn spelling(&self) -> String {
        let mut opts = self
            .opts
            .iter()
            .chain(&self.secondary)
            .cloned()
            .collect::<Vec<_>>();
        if !self.metavar.is_empty()
            && let Some(last) = opts.last_mut()
        {
            last.push_str(&format!(" <{}>", self.metavar));
        }
        opts.iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Command {
    pub path: Vec<String>,
    pub help: String,
    pub hidden: bool,
    pub params: Vec<Param>,
    pub children: Vec<Command>,
}
impl Command {
    pub fn label(&self) -> String {
        self.path.join(" ")
    }
    pub fn name(&self) -> &str {
        self.path.last().map_or("", String::as_str)
    }
    pub fn walk(&self) -> Vec<&Self> {
        if self.hidden || self.path.len() > 1 && self.name() == "help" {
            return Vec::new();
        }
        let mut found = vec![self];
        for child in &self.children {
            found.extend(child.walk());
        }
        found
    }
}

pub fn native() -> Command {
    let mut command = crate::cli::Cli::command();
    command.build();
    from_clap(&command, vec!["dotfile".into()])
}
fn from_clap(command: &clap::Command, path: Vec<String>) -> Command {
    let params = command
        .get_arguments()
        .map(|arg| {
            let mut opts = Vec::new();
            if let Some(short) = arg.get_short() {
                opts.push(format!("-{short}"));
            }
            if let Some(long) = arg.get_long() {
                opts.push(format!("--{long}"));
            }
            let secondary = arg
                .get_visible_aliases()
                .unwrap_or_default()
                .into_iter()
                .map(|s| format!("--{s}"))
                .collect();
            let takes = arg.get_action().takes_values();
            Param {
                kind: if arg.is_positional() {
                    "argument"
                } else {
                    "option"
                }
                .into(),
                name: arg.get_id().to_string(),
                opts,
                secondary,
                metavar: if takes {
                    arg.get_value_names()
                        .and_then(|n| n.first())
                        .map_or_else(|| arg.get_id().as_str().to_uppercase(), ToString::to_string)
                } else {
                    String::new()
                },
                help: arg.get_help().map_or(String::new(), ToString::to_string),
                multiple: matches!(arg.get_action(), clap::ArgAction::Append)
                    || arg.get_num_args().is_some_and(|n| n.max_values() > 1),
                required: arg.is_required_set(),
                hidden: arg.is_hide_set(),
                conflicts: command
                    .get_arg_conflicts_with(arg)
                    .iter()
                    .flat_map(|other| {
                        let mut spellings = Vec::new();
                        if let Some(short) = other.get_short() {
                            spellings.push(format!("-{short}"));
                        }
                        if let Some(long) = other.get_long() {
                            spellings.push(format!("--{long}"));
                        }
                        spellings
                    })
                    .collect(),
                choices: arg
                    .get_value_parser()
                    .possible_values()
                    .map(|v| {
                        v.filter(|v| !v.is_hide_set())
                            .map(|v| v.get_name().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect();
    let children = command
        .get_subcommands()
        .map(|child| {
            let mut p = path.clone();
            p.push(child.get_name().to_string());
            from_clap(child, p)
        })
        .collect();
    Command {
        path,
        help: command
            .get_about()
            .map_or(String::new(), ToString::to_string),
        hidden: command.is_hide_set(),
        params,
        children,
    }
}

#[derive(Default, Deserialize)]
pub struct PythonSurface {
    pub version: u32,
    pub source_fingerprint: String,
    pub commands: BTreeMap<String, Command>,
    pub completions: BTreeMap<String, String>,
}
pub fn python(context: &Context) -> Result<PythonSurface, String> {
    let path = context.root.join("config/command-surface.json");
    match fs::read(&path) {
        Ok(bytes) => {
            let surface: PythonSurface =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            if surface.version != 1 {
                return Err("unsupported Python command metadata version".into());
            }
            Ok(surface)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PythonSurface::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

pub fn binary(context: &Context, program: &str) -> Result<Option<PathBuf>, String> {
    let name = if program == "gdd" {
        "git-discard"
    } else {
        program
    };
    if let Some(manifest) = context.env("DOTFILE_DEV_BUILD_MANIFEST") {
        let manifest = PathBuf::from(manifest);
        let text = fs::read_to_string(&manifest)
            .map_err(|e| format!("read {}: {e}", manifest.display()))?;
        let mut selected = None;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let artifact: serde_json::Value =
                serde_json::from_str(line).map_err(|e| format!("{}: {e}", manifest.display()))?;
            if artifact["reason"] == "compiler-artifact"
                && artifact["target"]["name"] == name
                && let Some(path) = artifact["executable"].as_str()
            {
                selected = Some(PathBuf::from(path));
            }
        }
        // A prepared build is authoritative: never use an unrelated installed binary.
        return Ok(selected.filter(|path| path.is_file()));
    }
    let candidates = [
        context.root.join("scripts/rust/target/debug").join(name),
        context.root.join("scripts/rust/target/release").join(name),
        context.home.join(".local/bin").join(name),
    ];
    Ok(candidates
        .into_iter()
        .filter(|p| p.is_file())
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok()))
}

pub fn external(context: &Context, program: &str) -> Result<Option<Command>, String> {
    let Some(binary) = binary(context, program)? else {
        return Ok(None);
    };
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

pub fn python_inputs(root: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    fn walk(path: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        if !path.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() && entry.file_name() != "__pycache__" {
                walk(&path, out)?;
            } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "py") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut inputs = vec![root.join("scripts/python/pyproject.toml")];
    let mut sources = Vec::new();
    walk(&root.join("scripts/python/src"), &mut sources)?;
    sources.sort();
    inputs.extend(sources);
    Ok(inputs)
}
pub fn python_fingerprint(context: &Context) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for path in python_inputs(&context.root)? {
        if !path.is_file() {
            continue;
        }
        hash.update(
            path.strip_prefix(&context.root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .as_bytes(),
        );
        hash.update([0]);
        hash.update(fs::read(&path).map_err(|e| e.to_string())?);
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
