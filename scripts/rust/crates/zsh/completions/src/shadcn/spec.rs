use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::{self, Source};
use crate::context::{Context, fingerprint};
use crate::help::Help;
use crate::process;

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
        "shadcn-spec".into()
    }

    fn stamp(&self, _ctx: &Context) -> String {
        fingerprint(&self.binary)
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn job(&self) -> Vec<String> {
        vec!["shadcn-spec".into()]
    }

    fn build(&self, ctx: &Context) -> Option<Spec> {
        self.read(ctx, &[])
    }
}

impl SpecSource {
    fn read(&self, ctx: &Context, path: &[&str]) -> Option<Spec> {
        let mut args = path.to_vec();
        args.push("--help");
        let text = process::output(&self.binary, &args, &ctx.cwd, Duration::from_secs(10))?;
        let mut program = vec!["shadcn"];
        program.extend_from_slice(path);
        let help = Help::parse(&text, &program);
        if help.commands.is_empty() && help.flags.is_empty() {
            return None;
        }
        let mut children = BTreeMap::new();
        // Only inspect the command tree reported by help, with a bounded depth.
        if path.len() < 3 {
            for command in &help.commands {
                if command.name == "help" {
                    continue;
                }
                let mut nested = path.to_vec();
                nested.push(&command.name);
                children.insert(command.name.clone(), self.read(ctx, &nested)?);
            }
        }
        Some(Spec { help, children })
    }
}

pub fn load(ctx: &Context) -> Spec {
    ctx.which("shadcn")
        .and_then(|binary| cache::load(ctx, &SpecSource { binary }))
        .unwrap_or_default()
}
