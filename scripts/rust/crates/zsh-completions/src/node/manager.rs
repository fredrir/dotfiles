use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::{self, Source};
use crate::context::{Context, fingerprint};
use crate::help::Help;
use crate::process;

const HELP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Manager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Add,
    Remove,
    Update,
    Run,
    Dlx,
}

// The subcommand behind each role, with the aliases a tool may leave out of its help.
type RoleTable = &'static [(Role, &'static str, &'static [&'static str])];

const NPM: RoleTable = &[
    (
        Role::Add,
        "install",
        &[
            "i", "add", "in", "ins", "inst", "insta", "instal", "isnt", "isnta", "isntal",
            "isntall",
        ],
    ),
    (
        Role::Remove,
        "uninstall",
        &["unlink", "remove", "rm", "r", "un"],
    ),
    (Role::Update, "update", &["up", "upgrade", "udpate"]),
    (Role::Run, "run", &["run-script", "rum", "urn"]),
    (Role::Dlx, "exec", &["x"]),
];

const PNPM: RoleTable = &[
    (Role::Add, "add", &[]),
    (Role::Remove, "remove", &["rm", "uninstall", "un", "uni"]),
    (Role::Update, "update", &["up", "upgrade"]),
    (Role::Run, "run", &["run-script"]),
    (Role::Dlx, "dlx", &[]),
];

const YARN: RoleTable = &[
    (Role::Add, "add", &[]),
    (Role::Remove, "remove", &[]),
    (Role::Update, "upgrade", &["up"]),
    (Role::Run, "run", &[]),
    (Role::Dlx, "dlx", &[]),
];

const BUN: RoleTable = &[
    (Role::Add, "add", &["a", "install", "i"]),
    (Role::Remove, "remove", &["rm"]),
    (Role::Update, "update", &[]),
    (Role::Run, "run", &[]),
    (Role::Dlx, "x", &[]),
];

impl Manager {
    pub const ALL: [Manager; 4] = [Manager::Npm, Manager::Pnpm, Manager::Yarn, Manager::Bun];

    pub fn program(self) -> &'static str {
        match self {
            Manager::Npm => "npm",
            Manager::Pnpm => "pnpm",
            Manager::Yarn => "yarn",
            Manager::Bun => "bun",
        }
    }

    // Every command that runs this manager, and whether it runs packages instead.
    pub fn commands(self) -> &'static [(&'static str, bool)] {
        match self {
            Manager::Npm => &[("npm", false), ("npx", true)],
            Manager::Pnpm => &[
                ("pnpm", false),
                ("pn", false),
                ("pnpx", true),
                ("pnx", true),
            ],
            Manager::Yarn => &[("yarn", false)],
            Manager::Bun => &[("bun", false), ("bunx", true)],
        }
    }

    pub fn from_command(command: &str) -> Option<(Manager, bool)> {
        Manager::ALL.into_iter().find_map(|manager| {
            manager
                .commands()
                .iter()
                .find(|(name, _)| *name == command)
                .map(|(_, runner)| (manager, *runner))
        })
    }

    fn roles(self) -> RoleTable {
        match self {
            Manager::Npm => NPM,
            Manager::Pnpm => PNPM,
            Manager::Yarn => YARN,
            Manager::Bun => BUN,
        }
    }

    pub fn binary(self, ctx: &Context) -> Option<PathBuf> {
        self.commands()
            .iter()
            .filter(|(_, runner)| !runner)
            .find_map(|(name, _)| ctx.which(name))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Spec {
    pub top: Help,
    pub roles: Vec<RoleSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSpec {
    pub role: Role,
    pub command: String,
    pub aliases: Vec<String>,
    pub help: Help,
}

impl Spec {
    pub fn fallback(manager: Manager) -> Spec {
        Spec::assemble(manager, Help::default(), Vec::new())
    }

    fn assemble(manager: Manager, top: Help, helps: Vec<Help>) -> Spec {
        let roles = manager
            .roles()
            .iter()
            .enumerate()
            .map(|(index, (role, command, fallback))| {
                let help = helps.get(index).cloned().unwrap_or_default();
                let mut aliases: Vec<String> =
                    fallback.iter().map(|alias| alias.to_string()).collect();
                let listed = top.command(command).map(|known| known.aliases.clone());
                for alias in help
                    .aliases
                    .iter()
                    .cloned()
                    .chain(listed.into_iter().flatten())
                {
                    if alias != *command && !aliases.contains(&alias) {
                        aliases.push(alias);
                    }
                }
                RoleSpec {
                    role: *role,
                    command: command.to_string(),
                    aliases,
                    help,
                }
            })
            .collect();
        Spec { top, roles }
    }

    pub fn role_of(&self, subcommand: &str) -> Option<&RoleSpec> {
        self.roles.iter().find(|spec| {
            spec.command == subcommand || spec.aliases.iter().any(|alias| alias == subcommand)
        })
    }

    pub fn role(&self, role: Role) -> Option<&RoleSpec> {
        self.roles.iter().find(|spec| spec.role == role)
    }
}

pub struct SpecSource {
    pub manager: Manager,
    pub binary: PathBuf,
}

impl Source for SpecSource {
    type Value = Spec;

    fn key(&self) -> String {
        format!("node-spec-{}", self.manager.program())
    }

    fn stamp(&self, _ctx: &Context) -> String {
        fingerprint(&self.binary)
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn job(&self) -> Vec<String> {
        vec!["node-spec".into(), self.manager.program().into()]
    }

    fn build(&self, ctx: &Context) -> Option<Spec> {
        let program = [self.manager.program()];
        let commands: Vec<&str> = self
            .manager
            .roles()
            .iter()
            .map(|(_, command, _)| *command)
            .collect();
        let (top, helps) = thread::scope(|scope| {
            let top = scope.spawn(|| self.help(ctx, &[], &program));
            let helps: Vec<_> = commands
                .iter()
                .map(|command| {
                    scope.spawn(move || {
                        self.help(ctx, &[command], &[self.manager.program(), command])
                    })
                })
                .collect();
            let helps: Vec<Help> = helps
                .into_iter()
                .map(|handle| handle.join().ok().flatten().unwrap_or_default())
                .collect();
            (top.join().ok().flatten(), helps)
        });
        let top = top?;
        Some(Spec::assemble(self.manager, top, helps))
    }
}

impl SpecSource {
    fn help(&self, ctx: &Context, command: &[&str], program: &[&str]) -> Option<Help> {
        let mut args = command.to_vec();
        args.push("--help");
        let text = process::output(&self.binary, &args, &ctx.cwd, HELP_TIMEOUT)?;
        Some(Help::parse(&text, program))
    }
}

pub fn spec(ctx: &Context, manager: Manager) -> Spec {
    manager
        .binary(ctx)
        .and_then(|binary| cache::load(ctx, &SpecSource { manager, binary }))
        .unwrap_or_else(|| Spec::fallback(manager))
}

#[cfg(test)]
#[path = "../../tests/unit/node/manager_tests.rs"]
mod tests;
