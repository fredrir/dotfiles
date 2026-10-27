pub mod data;
pub mod spec;

use std::path::PathBuf;

use crate::cache;
use crate::context::Context;
use crate::help::{Flag, Help};
use crate::line::Line;
use crate::reply::{Group, Item, Reply, Tone};
use crate::shared;

pub fn complete(ctx: &Context, line: &Line) -> Reply {
    let spec = spec::load(ctx);
    let mut reply = Reply::new();
    let root = &spec.help;
    let deploy = spec.children.get("deploy");
    let mut current = &spec;
    let mut path: Vec<String> = Vec::new();
    let mut positionals = 0usize;
    let mut pending: Option<&Flag> = None;
    let mut after_separator = false;
    let mut help_mode = false;
    for word in line.before() {
        if let Some(flag) = pending.take()
            && (flag.takes_separate_value() || !word.starts_with('-'))
        {
            continue;
        }
        if !after_separator && word == "--" {
            after_separator = true;
            continue;
        }
        if !after_separator && word.starts_with('-') && word.len() > 1 {
            let leaf = level_help(root, current, deploy, path.is_empty());
            if shared::inline_value(leaf, word).is_none() {
                pending = leaf
                    .flag(word)
                    .or_else(|| root.flag(word))
                    .filter(|flag| flag.value.is_some());
            }
            continue;
        }
        if !after_separator && positionals == 0 {
            if word == "help" && current.help.command("help").is_some() {
                help_mode = true;
                continue;
            }
            if path.is_empty()
                && let Some(target) = root_path(word)
                && let Some(child) = resolve(&spec, target)
            {
                path.extend(target.iter().map(|name| (*name).to_string()));
                current = child;
                continue;
            }
            if let Some(command) = current.help.command(word)
                && let Some(child) = current.children.get(&command.name)
            {
                path.push(command.name.clone());
                current = child;
                continue;
            }
        }
        positionals += 1;
    }
    let prefix = line.prefix.as_str();
    let effective = effective_path(&path, positionals, prefix, current);
    let leaf = level_help(root, current, deploy, path.is_empty());
    let flags = merged(root, leaf);
    if help_mode {
        if positionals == 0 {
            commands(&current.help, &mut reply);
        }
    } else if let Some(flag) = pending
        && (flag.takes_separate_value() || !prefix.starts_with('-'))
    {
        value(ctx, flag, prefix, "", &effective, &mut reply);
    } else if !after_separator && prefix.starts_with('-') {
        if let Some((flag, text, leader)) = shared::inline_value(&flags, prefix) {
            value(ctx, flag, text, leader, &effective, &mut reply);
        } else if !prefix.contains('=') {
            shared::flags(&flags, &mut reply);
        }
    } else if after_separator {
        // After `--` the remaining words are positionals; `env run` still gets none.
        positional_values(ctx, &effective, positionals, &mut reply);
    } else if path.is_empty() && positionals == 0 {
        commands(root, &mut reply);
        reply.directories();
    } else if positionals == 0 && !current.help.commands.is_empty() {
        commands(&current.help, &mut reply);
        positional_values(ctx, &effective, positionals, &mut reply);
    } else {
        positional_values(ctx, &effective, positionals, &mut reply);
    }
    reply
}

fn level_help<'a>(
    root: &'a Help,
    current: &'a spec::Spec,
    deploy: Option<&'a spec::Spec>,
    root_path: bool,
) -> &'a Help {
    if root_path {
        deploy.map_or(root, |deploy| &deploy.help)
    } else {
        &current.help
    }
}

// The canonical path a command means, including default subcommands and root aliases.
fn effective_path(
    path: &[String],
    positionals: usize,
    prefix: &str,
    current: &spec::Spec,
) -> Vec<String> {
    let mut effective = path.to_vec();
    match path {
        [name] if name == "switch" => {
            effective = vec!["teams".into(), "switch".into()];
        }
        [name] if name == "install" => {
            effective = vec!["integration".into(), "add".into()];
        }
        [name]
            if name == "alias"
                && (positionals > 0
                    || (!prefix.is_empty() && current.help.command(prefix).is_none())) =>
        {
            effective.push("set".into());
        }
        _ => {}
    }
    effective
}

fn root_path(word: &str) -> Option<&'static [&'static str]> {
    match word {
        "switch" => Some(&["teams", "switch"]),
        "install" => Some(&["integration", "add"]),
        _ => None,
    }
}

fn resolve<'a>(root: &'a spec::Spec, path: &[&str]) -> Option<&'a spec::Spec> {
    let mut current = root;
    for name in path {
        current = current.children.get(*name)?;
    }
    Some(current)
}

fn merged(root: &Help, leaf: &Help) -> Help {
    let mut help = root.clone();
    for flag in &leaf.flags {
        help.add_flag(flag.clone());
    }
    help
}

fn commands(help: &Help, reply: &mut Reply) {
    reply.group(
        Group::new("commands", "vercel command").items(help.commands.iter().flat_map(|command| {
            std::iter::once(&command.name)
                .chain(&command.aliases)
                .map(|name| Item::new(name, &command.description))
        })),
    );
}

fn positional_values(ctx: &Context, path: &[String], index: usize, reply: &mut Reply) {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    match (path.as_slice(), index) {
        ([] | ["deploy"] | ["build"] | ["dev"] | ["pull"], 0) => paths(reply, true),
        (["env", "pull"], 0) => paths(reply, false),
        (["teams", "switch"] | ["switch"], 0) => teams(ctx, reply),
        (["domains", "move"], 1) => teams(ctx, reply),
        _ => {}
    }
}

// An existing `_vercel` completion gets the first chance at paths.
fn paths(reply: &mut Reply, directories: bool) {
    reply.delegate();
    if directories {
        reply.directories();
    } else {
        reply.files();
    }
}

fn teams(ctx: &Context, reply: &mut Reply) {
    let teams = data::teams(ctx);
    if !teams.is_empty() {
        teams_group(teams, reply);
    }
}

fn teams_group(teams: Vec<data::Team>, reply: &mut Reply) {
    reply.group(
        Group::new("teams", "team")
            .unsorted()
            .items(teams.into_iter().map(|team| {
                let item = Item::new(team.slug, team.name);
                if team.current {
                    item.detail(Tone::Success, "current")
                } else {
                    item
                }
            })),
    );
}

fn value(
    ctx: &Context,
    flag: &Flag,
    prefix: &str,
    leader: &str,
    path: &[String],
    reply: &mut Reply,
) {
    reply.skip(leader);
    let env_path = path.first().is_some_and(|command| command == "env");
    if flag.has("--scope")
        || flag.has("-S")
        || (flag.has("--team") && path.first().is_some_and(|command| command == "link"))
    {
        teams(ctx, reply);
    } else if flag.has("--cwd") || flag.has("--global-config") {
        reply.directories();
    } else if flag.has("--local-config") {
        reply.files();
    } else if flag.has("--project") {
        reply.message("project name");
    } else if flag.has("--env") {
        reply.message("KEY=VALUE");
    } else if flag.has("--status") || (flag.has("--environment") && env_path) {
        list_value(flag, prefix, leader, reply);
    } else {
        shared::generic_value(flag, reply);
    }
}

fn list_value(flag: &Flag, prefix: &str, leader: &str, reply: &mut Reply) {
    let Some(value) = &flag.value else { return };
    if value.choices.is_empty() {
        shared::generic_value(flag, reply);
        return;
    }
    let (element, chosen) = shared::list_element(prefix, reply);
    let before = &prefix[..prefix.len() - element.len()];
    reply.skip(format!("{leader}{before}"));
    reply.group(
        Group::new("values", &value.placeholder)
            .suffix(",", true)
            .items(
                value
                    .choices
                    .iter()
                    .filter(|choice| !chosen.contains(&choice.as_str()))
                    .map(Item::bare),
            ),
    );
}

pub fn binary(ctx: &Context) -> Option<PathBuf> {
    ctx.which("vercel").or_else(|| ctx.which("vc"))
}

pub fn warm(ctx: &Context) {
    let Some(binary) = binary(ctx) else { return };
    cache::warm(
        ctx,
        &spec::SpecSource {
            binary: binary.clone(),
        },
    );
    if !ctx.offline && data::auth_dir(ctx).is_some_and(|dir| dir.join("auth.json").is_file()) {
        cache::warm(ctx, &data::TeamsSource { binary });
    }
}

pub fn refresh(ctx: &Context, source: &str, _args: &[String]) -> bool {
    let Some(binary) = binary(ctx) else {
        return false;
    };
    match source {
        "vercel-spec" => cache::refresh(ctx, &spec::SpecSource { binary }),
        "vercel-teams" => cache::refresh(ctx, &data::TeamsSource { binary }),
        _ => false,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/vercel_tests.rs"]
mod tests;
