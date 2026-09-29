pub mod gallery;
pub mod models;
pub mod resources;
pub mod sessions;
pub mod spec;

use std::collections::BTreeMap;

use crate::context::{Context, age};
use crate::help::{Flag, Help};
use crate::line::{Line, scan};
use crate::node::npmrc::Npmrc;
use crate::node::registry;
use crate::reply::{Group, Item, Reply, Tone};
use crate::shared;
use models::Model;
use resources::Settings;
use spec::{Pi, Spec};

const SCHEMES: [(&str, &str); 4] = [
    ("npm:", "npm package"),
    ("git:", "git repository"),
    ("https://", "git repository URL"),
    ("ssh://", "git repository over SSH"),
];
const EMPTY_PREFIX_LIMIT: usize = 40;

pub fn complete(ctx: &Context, line: &Line) -> Reply {
    let pi = Pi::locate(ctx);
    let spec = spec::load(ctx, &pi);
    let mut reply = Reply::new();
    Completer {
        ctx,
        pi: &pi,
        spec: &spec,
        line,
    }
    .run(&mut reply);
    reply
}

struct Completer<'a> {
    ctx: &'a Context,
    pi: &'a Pi,
    spec: &'a Spec,
    line: &'a Line,
}

impl Completer<'_> {
    fn run(&self, reply: &mut Reply) {
        let main = &self.spec.main;
        let before = self.line.before();
        let prefix = self.line.prefix.as_str();
        let top = scan(before, &[main]);
        let command = top
            .positionals
            .first()
            .copied()
            .filter(|word| main.command(word).is_some());
        let (help, words) = match command {
            Some(name) => {
                let at = before.iter().position(|word| word == name).unwrap_or(0);
                (self.spec.command(name).unwrap_or(main), &before[at + 1..])
            }
            None => (main, before),
        };
        let scanned = scan(words, &[help]);
        if let Some(flag) = scanned.pending.or_else(|| self.optional_value_flag(help)) {
            self.value(flag, prefix, reply);
            return;
        }
        if prefix.starts_with('-') && !scanned.after_separator {
            if let Some((name, value)) = prefix.split_once('=') {
                if let Some(flag) = help.flag(name) {
                    reply.skip(format!("{name}="));
                    self.value(flag, value, reply);
                }
                return;
            }
            shared::flags(help, reply);
            return;
        }
        let Some(name) = command else {
            if prefix.starts_with('@') {
                reply.skip("@");
                reply.files();
            } else if scanned.positionals.is_empty() {
                reply.group(
                    Group::new("commands", "pi command").items(
                        main.commands
                            .iter()
                            .map(|command| Item::new(&command.name, &command.description)),
                    ),
                );
            }
            return;
        };
        let canonical = main
            .command(name)
            .map_or(name, |command| command.name.as_str());
        self.argument(canonical, help, scanned.positionals.len(), reply);
    }

    // `--list-models [search]` takes its value only when one follows.
    fn optional_value_flag<'h>(&self, help: &'h Help) -> Option<&'h Flag> {
        let flag = help.flag(self.line.previous()?)?;
        let value = flag.value.as_ref()?;
        (value.optional && flag.long() == Some("list-models")).then_some(flag)
    }

    fn argument(&self, command: &str, help: &Help, index: usize, reply: &mut Reply) {
        if index > 0 {
            return;
        }
        if !help.commands.is_empty() {
            reply.group(
                Group::new("commands", &format!("{command} command")).items(
                    help.commands
                        .iter()
                        .map(|command| Item::new(&command.name, &command.description)),
                ),
            );
            return;
        }
        match command {
            "install" => self.install_source(reply),
            "remove" | "uninstall" => self.package_sources(reply),
            "update" => {
                reply.group(
                    Group::new("targets", "update target").items(
                        help.positionals
                            .iter()
                            .map(|word| Item::new(word, "pi itself")),
                    ),
                );
                self.package_sources(reply);
            }
            _ => {}
        }
    }

    fn value(&self, flag: &Flag, prefix: &str, reply: &mut Reply) {
        let placeholder = flag
            .value
            .as_ref()
            .map_or("", |value| value.placeholder.as_str());
        match flag.long().unwrap_or("") {
            "provider" => self.providers(reply),
            "model" => self.model_with_thinking(prefix, reply),
            "models" => {
                let (element, _) = shared::list_element(prefix, reply);
                self.models(element, reply, ",");
            }
            "list-models" => self.models(prefix, reply, " "),
            "session" | "fork" => self.sessions(prefix, reply),
            "tools" | "exclude-tools" => self.tools(prefix, reply),
            "use-theme" => self.themes(prefix, reply),
            "append-system-prompt" | "system-prompt" => reply.files(),
            _ if placeholder == "source" => self.package_sources(reply),
            _ => shared::generic_value(flag, reply),
        }
    }

    fn models_cached(&self) -> Vec<Model> {
        models::load(self.ctx, self.pi)
    }

    fn providers(&self, reply: &mut Reply) {
        let settings = Settings::load(self.ctx, self.pi);
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for model in self.models_cached() {
            *counts.entry(model.provider).or_default() += 1;
        }
        reply.group(
            Group::new("providers", "provider").items(counts.into_iter().map(
                |(provider, count)| {
                    let default = settings.default_provider.as_deref() == Some(provider.as_str());
                    let models = format!("{count} model{}", if count == 1 { "" } else { "s" });
                    Item::new(provider, models)
                        .detail(Tone::Success, if default { "default" } else { "" })
                },
            )),
        );
    }

    fn model_with_thinking(&self, prefix: &str, reply: &mut Reply) {
        let Some((model, _)) = prefix.rsplit_once(':') else {
            self.models(prefix, reply, " ");
            return;
        };
        reply.skip(format!("{model}:"));
        let levels = self.thinking_levels();
        let models = self.models_cached();
        let (provider, id) = model
            .split_once('/')
            .map_or((None, model), |(p, id)| (Some(p), id));
        let found = models.iter().find(|known| {
            known.id == id && provider.is_none_or(|provider| known.provider == provider)
        });
        let supported = match found {
            Some(model) => model.thinking_levels(&levels),
            None => levels,
        };
        reply.group(
            Group::new("thinking", "thinking level")
                .unsorted()
                .items(supported.into_iter().map(Item::bare)),
        );
    }

    fn thinking_levels(&self) -> Vec<String> {
        self.spec
            .main
            .flag("--thinking")
            .and_then(|flag| flag.value.as_ref())
            .map(|value| value.choices.clone())
            .unwrap_or_default()
    }

    fn models(&self, prefix: &str, reply: &mut Reply, suffix: &str) {
        let settings = Settings::load(self.ctx, self.pi);
        let models = self.models_cached();
        let given_provider = self.line.flag_value(&["--provider"]);
        let describe = |model: &Model, with_provider: bool| {
            let provider = if with_provider {
                model.provider.as_str()
            } else {
                ""
            };
            let default = settings.default_model.as_deref() == Some(model.id.as_str())
                && settings
                    .default_provider
                    .as_deref()
                    .is_none_or(|p| p == model.provider);
            Item::bare(&model.id)
                .detail(Tone::Info, provider)
                .detail(Tone::Muted, &model.name)
                .detail(Tone::Success, if default { "default" } else { "" })
        };
        let listed = |group: Group| {
            if suffix == "," {
                group.suffix(",", true)
            } else {
                group
            }
        };
        if let Some((provider, _)) = prefix.split_once('/') {
            reply.skip(format!("{provider}/"));
            reply.group(
                listed(Group::new("models", &format!("{provider} model"))).items(
                    models
                        .iter()
                        .filter(|model| model.provider == provider)
                        .map(|model| describe(model, false)),
                ),
            );
            return;
        }
        let only = given_provider.filter(|provider| models.iter().any(|m| m.provider == *provider));
        reply.group(
            listed(Group::new("models", "model")).items(
                models
                    .iter()
                    .filter(|model| only.is_none_or(|provider| model.provider == provider))
                    .map(|model| describe(model, only.is_none())),
            ),
        );
        if only.is_none() {
            let mut providers: Vec<&str> =
                models.iter().map(|model| model.provider.as_str()).collect();
            providers.sort_unstable();
            providers.dedup();
            reply.group(
                Group::new("providers", "provider/model")
                    .tone(Tone::Info)
                    .suffix("", false)
                    .items(
                        providers
                            .into_iter()
                            .map(|provider| Item::new(format!("{provider}/"), "provider")),
                    ),
            );
        }
    }

    fn sessions(&self, prefix: &str, reply: &mut Reply) {
        if prefix.contains('/') || prefix.ends_with(".jsonl") || prefix.starts_with(['.', '~']) {
            reply.files();
            return;
        }
        let flag = self.line.flag_value(&["--session-dir"]);
        let (dir, shared_dir) = sessions::directory(self.ctx, self.pi, flag);
        let now = self.ctx.now_secs();
        let found = sessions::list(self.ctx, &dir, shared_dir);
        reply.group(
            Group::new("sessions", "session")
                .unsorted()
                .items(found.into_iter().map(|session| {
                    let summary = session.name.or(session.first_message).unwrap_or_default();
                    Item::new(session.id, age(now, session.modified)).detail(Tone::Plain, summary)
                })),
        );
    }

    fn tools(&self, prefix: &str, reply: &mut Reply) {
        let (_, chosen) = shared::list_element(prefix, reply);
        let mut tools = resources::tools(self.pi);
        tools.retain(|tool| !chosen.contains(&tool.as_str()));
        if tools.is_empty() && chosen.is_empty() {
            reply.message("tool names, separated by commas");
            return;
        }
        reply.group(
            Group::new("tools", "tool")
                .suffix(",", true)
                .items(tools.into_iter().map(|tool| Item::new(tool, "built-in"))),
        );
    }

    fn themes(&self, prefix: &str, reply: &mut Reply) {
        if let Some(at) = prefix.rfind('/') {
            reply.skip(&prefix[..=at]);
        }
        let settings = Settings::load(self.ctx, self.pi);
        let themes = resources::themes(self.ctx, self.pi, &settings);
        reply.group(
            Group::new("themes", "theme").items(
                themes
                    .into_iter()
                    .map(|theme| Item::new(theme.name, theme.origin)),
            ),
        );
    }

    fn package_sources(&self, reply: &mut Reply) {
        let settings = Settings::load(self.ctx, self.pi);
        reply.group(Group::new("sources", "installed package").items(
            settings.packages.into_iter().map(|package| {
                let scope = if package.project { "project" } else { "global" };
                Item::new(package.source, scope)
            }),
        ));
    }

    fn install_source(&self, reply: &mut Reply) {
        let prefix = self.line.prefix.as_str();
        if let Some(rest) = prefix.strip_prefix("npm:") {
            if let Some((name, _)) = registry::split_version(rest) {
                let registry = Npmrc::load(self.ctx).registry_for(name);
                let skip = format!("npm:{name}@");
                crate::node::versions(self.ctx, &registry, name, &skip, reply);
                return;
            }
            reply.skip("npm:");
            self.packages(rest, None, reply);
            return;
        }
        if ["git:", "http://", "https://", "ssh://"]
            .iter()
            .any(|scheme| prefix.starts_with(scheme))
        {
            reply.message("a git repository, optionally pinned with @ref");
            return;
        }
        if prefix.starts_with(['.', '/', '~']) {
            reply.files();
            return;
        }
        self.packages(prefix, Some("npm:"), reply);
        reply.group(
            Group::new("schemes", "source type")
                .tone(Tone::Info)
                .unsorted()
                .suffix("", false)
                .items(SCHEMES.map(|(scheme, description)| Item::new(scheme, description))),
        );
    }

    // The pi gallery first, most downloaded leading, then the rest of the registry.
    fn packages(&self, text: &str, insert: Option<&str>, reply: &mut Reply) {
        let gallery = gallery::packages(self.ctx);
        let (gallery, searched) = if text.is_empty() {
            (
                gallery.into_iter().take(EMPTY_PREFIX_LIMIT).collect(),
                Vec::new(),
            )
        } else {
            let registry = Npmrc::load(self.ctx).registry_for(text);
            (
                registry::rank(gallery, text),
                registry::search(self.ctx, &registry, text),
            )
        };
        let with_insert = |group: Group| match insert {
            Some(insert) => group.insert_prefix(insert),
            None => group,
        };
        reply.group(crate::node::found(
            with_insert(Group::new("gallery", "pi package")),
            gallery,
        ));
        reply.group(crate::node::found(
            with_insert(Group::new("registry", "npm package")),
            searched,
        ));
    }
}

#[cfg(test)]
#[path = "../../tests/unit/pi/mod_tests.rs"]
mod tests;
