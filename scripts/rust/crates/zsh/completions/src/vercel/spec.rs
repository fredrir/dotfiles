use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::{self, FirstUse, Source};
use crate::context::{Context, fingerprint};
use crate::help::{Flag, Help, Value};
use crate::process;

const HELP_TIMEOUT: Duration = Duration::from_secs(5);
const WORKERS: usize = 4;
const CURATED_PARENTS: &[&str] = &[
    "alias", "domains", "env", "project", "teams", "storage", "link", "promote", "rollback",
];

// Root help leaves aliases out, so they come from the CLI's command table.
const ALIASES: &[(&str, &[&str])] = &[
    ("alias", &["aliases", "ln"]),
    ("budgets", &["budget"]),
    ("certs", &["cert"]),
    ("crons", &["cron"]),
    ("daemons", &["daemon"]),
    ("deploy-hooks", &["deploy-hook"]),
    ("dev", &["develop"]),
    ("domains", &["domain"]),
    ("environments", &["environment", "targets", "target"]),
    ("global-config", &["edge-config"]),
    ("health", &["status-page"]),
    ("install", &["i"]),
    ("list", &["ls"]),
    ("logs", &["log"]),
    ("microfrontends", &["mf"]),
    ("organizations", &["organization"]),
    ("project", &["projects"]),
    ("redirects", &["redirect"]),
    ("remove", &["rm"]),
    ("rolling-release", &["rr"]),
    ("teams", &["switch", "team"]),
    ("token-pass", &["tokenpass"]),
    ("version", &["versions"]),
    ("webhooks", &["webhook"]),
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Spec {
    pub help: Help,
    pub children: BTreeMap<String, Spec>,
}

pub struct SpecSource {
    pub binary: PathBuf,
}

impl Source for SpecSource {
    type Value = Spec;

    fn key(&self) -> String {
        "vercel-spec".into()
    }

    fn stamp(&self, _ctx: &Context) -> String {
        fingerprint(&self.binary)
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn job(&self) -> Vec<String> {
        vec!["vercel-spec".into()]
    }

    fn first_use(&self) -> FirstUse {
        FirstUse::Background
    }

    fn build(&self, ctx: &Context) -> Option<Spec> {
        let root = self.help(ctx, &[])?;
        // Aliases are injected afterwards, so each canonical command is fetched once.
        let names: Vec<String> = root
            .commands
            .iter()
            .map(|command| command.name.clone())
            .filter(|name| name != "help")
            .collect();
        let built = thread::scope(|scope| {
            let mut built = Vec::new();
            for chunk in names.chunks(WORKERS) {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|name| scope.spawn(move || (name.clone(), self.read(ctx, name))))
                    .collect();
                for handle in handles {
                    if let Ok(item) = handle.join() {
                        built.push(item);
                    }
                }
            }
            built
        });
        let children = built
            .into_iter()
            .filter_map(|(name, child)| Some((name, child?)))
            .collect();
        let mut spec = Spec {
            help: root,
            children,
        };
        inject_aliases(&mut spec.help);
        Some(spec)
    }
}

impl SpecSource {
    fn read(&self, ctx: &Context, name: &str) -> Option<Spec> {
        let help = self.help(ctx, &[name])?;
        let mut children = BTreeMap::new();
        if CURATED_PARENTS.contains(&name) {
            let names: Vec<String> = help
                .commands
                .iter()
                .map(|command| command.name.clone())
                .filter(|name| name != "help")
                .collect();
            for child in names {
                if let Some(help) = self.help(ctx, &[name, child.as_str()]) {
                    children.insert(
                        child,
                        Spec {
                            help,
                            children: BTreeMap::new(),
                        },
                    );
                }
            }
        }
        Some(Spec { help, children })
    }

    fn help(&self, ctx: &Context, path: &[&str]) -> Option<Help> {
        let mut args = path.to_vec();
        args.push("--help");
        let text = process::output(&self.binary, &args, &ctx.cwd, HELP_TIMEOUT)?;
        let mut program = vec!["vercel"];
        program.extend_from_slice(path);
        let mut help = Help::parse(&text, &program);
        patch(&mut help, path);
        Some(help)
    }
}

fn patch(help: &mut Help, path: &[&str]) {
    for flag in &mut help.flags {
        if (flag.has("--scope") || flag.has("-S")) && flag.value.is_none() {
            flag.value = Some(Value::new("team", false));
        }
        if flag.has("--cwd") && flag.value.is_none() {
            flag.value = Some(Value::new("DIR", false));
        }
        if path.first() == Some(&"link") && flag.has("--team") {
            flag.value = Some(Value::new("team", false));
        }
    }
    if let Some(flag) = help.flags.iter_mut().find(|flag| flag.has("--status")) {
        set_choices(
            flag,
            &[
                "BUILDING",
                "ERROR",
                "INITIALIZING",
                "QUEUED",
                "READY",
                "CANCELED",
                "BLOCKED",
            ],
        );
    }
    if let Some(flag) = help.flags.iter_mut().find(|flag| flag.has("--archive")) {
        set_choices(flag, &["tgz"]);
    }
    if matches!(path, ["env", "add"] | ["env", "update"])
        && let Some(flag) = help.flags.iter_mut().find(|flag| flag.has("--type"))
    {
        set_choices(flag, &["config", "secret"]);
    }
    if let Some(flag) = help.flags.iter_mut().find(|flag| flag.has("--environment")) {
        if path.first() == Some(&"env") {
            set_choices(flag, &["production", "preview", "development"]);
        } else {
            set_choices(flag, &["production", "preview"]);
        }
    }
}

fn set_choices(flag: &mut Flag, choices: &[&str]) {
    if let Some(value) = flag.value.as_mut() {
        value.choices = choices.iter().map(|choice| (*choice).to_string()).collect();
    }
}

fn inject_aliases(help: &mut Help) {
    for (canonical, aliases) in ALIASES {
        let Some(command) = help
            .commands
            .iter_mut()
            .find(|command| command.name.as_str() == *canonical)
        else {
            continue;
        };
        for alias in *aliases {
            if *alias != *canonical && !command.aliases.iter().any(|known| known.as_str() == *alias)
            {
                command.aliases.push((*alias).to_string());
            }
        }
    }
}

pub fn load(ctx: &Context) -> Spec {
    super::binary(ctx)
        .and_then(|binary| cache::load(ctx, &SpecSource { binary }))
        .unwrap_or_default()
}
