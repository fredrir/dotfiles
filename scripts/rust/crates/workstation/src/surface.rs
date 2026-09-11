use serde::{Deserialize, Serialize};

pub mod zsh;

pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub version: u32,
    pub command: Command,
}

pub fn document(command: &clap::Command, program: &str) -> Document {
    Document {
        version: VERSION,
        command: from_clap(command, vec![program.into()]),
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<Completion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<char>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Completion {
    Call {
        source: String,
    },
    Files {
        #[serde(default)]
        pattern: String,
    },
    Dirs,
    Pair {
        groups: String,
        names: String,
    },
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
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate: Option<String>,
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

pub fn from_clap(command: &clap::Command, path: Vec<String>) -> Command {
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
                delimiter: arg.get_value_delimiter(),
                completion: match arg.get_value_hint() {
                    clap::ValueHint::DirPath => Some(Completion::Dirs),
                    clap::ValueHint::AnyPath
                    | clap::ValueHint::FilePath
                    | clap::ValueHint::ExecutablePath => Some(Completion::Files {
                        pattern: String::new(),
                    }),
                    _ => None,
                },
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
        delegate: None,
        aliases: command.get_visible_aliases().map(str::to_string).collect(),
        path,
        help: command
            .get_about()
            .map_or(String::new(), ToString::to_string),
        hidden: command.is_hide_set(),
        params,
        children,
    }
}
