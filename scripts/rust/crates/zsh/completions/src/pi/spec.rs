use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::{self, Source};
use crate::context::{Context, fingerprint};
use crate::help::Help;
use crate::process;

const HELP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct Pi {
    pub binary: Option<PathBuf>,
    // The installed coding-agent package, next to the model catalogs and built-in themes.
    pub package: Option<PathBuf>,
    pub agent_dir: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Spec {
    pub main: Help,
    pub commands: Vec<(String, Help)>,
}

impl Pi {
    pub fn locate(ctx: &Context) -> Pi {
        let binary = ctx.which("pi");
        let package = binary
            .as_ref()
            .and_then(|binary| fs::canonicalize(binary).ok())
            .and_then(|resolved| {
                resolved
                    .ancestors()
                    .find(|dir| dir.join("package.json").is_file())
                    .map(Path::to_path_buf)
            });
        let agent_dir = ctx
            .var_path("PI_CODING_AGENT_DIR")
            .unwrap_or_else(|| ctx.home.join(".pi/agent"));
        Pi {
            binary,
            package,
            agent_dir,
        }
    }

    pub fn project_dir(ctx: &Context) -> PathBuf {
        ctx.cwd.join(".pi")
    }

    pub fn settings_files(&self, ctx: &Context) -> [PathBuf; 2] {
        [
            Pi::project_dir(ctx).join("settings.json"),
            self.agent_dir.join("settings.json"),
        ]
    }
}

impl Spec {
    pub fn command(&self, name: &str) -> Option<&Help> {
        let canonical = self
            .main
            .command(name)
            .map_or(name, |command| command.name.as_str());
        self.commands
            .iter()
            .find(|(known, _)| known == canonical || known == name)
            .map(|(_, help)| help)
    }

    pub fn from_texts(main: &str, commands: &[(&str, &str)]) -> Spec {
        Spec {
            main: Help::parse(main, &["pi"]),
            commands: commands
                .iter()
                .map(|(name, text)| (name.to_string(), Help::parse(text, &["pi", name])))
                .collect(),
        }
    }
}

pub struct SpecSource<'a> {
    pub pi: &'a Pi,
    pub binary: PathBuf,
}

impl Source for SpecSource<'_> {
    type Value = Spec;

    fn key(&self) -> String {
        "pi-spec".into()
    }

    // Extensions add flags, so settings that change what loads also change the spec.
    fn stamp(&self, ctx: &Context) -> String {
        let mut stamp = fingerprint(&self.binary);
        for file in self.pi.settings_files(ctx) {
            stamp.push('|');
            stamp.push_str(&fingerprint(&file));
        }
        stamp
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn job(&self) -> Vec<String> {
        vec!["pi-spec".into()]
    }

    fn build(&self, ctx: &Context) -> Option<Spec> {
        let main = self.help(ctx, &[])?;
        let names: Vec<String> = Help::parse(&main, &["pi"])
            .commands
            .into_iter()
            .map(|command| command.name)
            .collect();
        let texts: Vec<(String, String)> = thread::scope(|scope| {
            let handles: Vec<_> = names
                .iter()
                .map(|name| scope.spawn(move || (name.clone(), self.help(ctx, &[name]))))
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .filter_map(|(name, text)| text.map(|text| (name, text)))
                .collect()
        });
        let borrowed: Vec<(&str, &str)> = texts
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str()))
            .collect();
        Some(Spec::from_texts(&main, &borrowed))
    }
}

impl SpecSource<'_> {
    fn help(&self, ctx: &Context, command: &[&str]) -> Option<String> {
        let mut args = command.to_vec();
        args.extend(["--offline", "--help"]);
        process::output(&self.binary, &args, &ctx.cwd, HELP_TIMEOUT)
    }
}

pub fn load(ctx: &Context, pi: &Pi) -> Spec {
    pi.binary
        .clone()
        .and_then(|binary| cache::load(ctx, &SpecSource { pi, binary }))
        .unwrap_or_default()
}
