pub mod spec;

use crate::context::Context;
use crate::help::{Flag, Help};
use crate::line::Line;
use crate::reply::{Group, Item, Reply};
use crate::shared;

// Built-in UI components; custom registry addresses can still be entered directly.
const COMPONENTS: &str = "accordion alert alert-dialog aspect-ratio attachment avatar badge \
    breadcrumb bubble button button-group calendar card carousel chart checkbox collapsible \
    combobox command context-menu dialog direction drawer dropdown-menu empty field form \
    hover-card input input-group input-otp item kbd label marker menubar message message-scroller \
    native-select navigation-menu pagination popover progress questionnaire radio-group resizable \
    scroll-area select separator sheet sidebar skeleton slider sonner spinner switch table tabs \
    textarea toast toggle toggle-group tooltip";

pub fn complete(ctx: &Context, line: &Line) -> Reply {
    let spec = spec::load(ctx);
    complete_with(&spec, line)
}

fn complete_with(root: &spec::Spec, line: &Line) -> Reply {
    let mut reply = Reply::new();
    let mut spec = root;
    let mut path = Vec::new();
    let mut positionals = 0;
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
        if !after_separator && word.starts_with('-') {
            if inline_value(&spec.help, word).is_none() {
                pending = spec.help.flag(word).filter(|flag| flag.value.is_some());
            }
            continue;
        }
        if positionals == 0 && !after_separator {
            if word == "help" && spec.help.command("help").is_some() {
                help_mode = true;
                continue;
            }
            if let Some(command) = spec.help.command(word)
                && let Some(child) = spec.children.get(&command.name)
            {
                path.push(command.name.as_str());
                spec = child;
                continue;
            }
        }
        positionals += 1;
    }
    let prefix = line.prefix.as_str();
    if let Some(flag) = pending
        && (flag.takes_separate_value() || !prefix.starts_with('-'))
    {
        value(flag, prefix, "", &mut reply);
    } else if !after_separator && !help_mode && prefix.starts_with('-') {
        if let Some((flag, text, leader)) = inline_value(&spec.help, prefix) {
            value(flag, text, leader, &mut reply);
        } else if !prefix.contains('=') {
            shared::flags(&spec.help, &mut reply);
        }
    } else if positionals == 0 && !after_separator && !spec.help.commands.is_empty() {
        commands(&spec.help, &mut reply);
    } else if !help_mode {
        match path.as_slice() {
            ["init" | "add" | "docs" | "view"] => components(&mut reply),
            ["diff"] if positionals == 0 => components(&mut reply),
            ["search"] | ["registry", "add"] => {
                choices("registry", "@shadcn", &mut reply);
            }
            ["migrate"] if positionals == 0 => {
                choices("migration", "cn icons base-color radix rtl", &mut reply);
            }
            ["migrate"] if positionals == 1 => reply.files(),
            ["build"] | ["registry", "validate"] if positionals == 0 => reply.files(),
            ["apply"] if positionals == 0 => reply.message("preset name, code or URL"),
            ["preset", "decode" | "url" | "open"] if positionals == 0 => {
                reply.message("preset code");
            }
            _ => {}
        }
    }
    reply
}

fn commands(help: &Help, reply: &mut Reply) {
    reply.group(
        Group::new("commands", "shadcn command").items(help.commands.iter().flat_map(|command| {
            std::iter::once(&command.name)
                .chain(&command.aliases)
                .map(|name| Item::new(name, &command.description))
        })),
    );
}

// Commander accepts --flag=value and attached short values such as -c/path.
fn inline_value<'a, 'w>(help: &'a Help, word: &'w str) -> Option<(&'a Flag, &'w str, &'w str)> {
    if let Some((name, text)) = word.split_once('=') {
        let flag = help.flag(name).filter(|flag| flag.value.is_some())?;
        return Some((flag, text, &word[..name.len() + 1]));
    }
    help.flags.iter().find_map(|flag| {
        flag.value.as_ref()?;
        flag.names.iter().find_map(|name| {
            (name.len() == 2 && word.starts_with(name) && word.len() > 2)
                .then(|| (flag, &word[2..], &word[..2]))
        })
    })
}

fn components(reply: &mut Reply) {
    reply.group(
        Group::new("components", "component").items(COMPONENTS.split_whitespace().map(Item::bare)),
    );
    reply.files();
}

fn choices(label: &str, values: &str, reply: &mut Reply) {
    reply.group(Group::new("values", label).items(values.split_whitespace().map(Item::bare)));
}

fn value(flag: &Flag, prefix: &str, leader: &str, reply: &mut Reply) {
    reply.skip(leader);
    match flag.long().unwrap_or("") {
        "cwd" | "path" | "output" => reply.directories(),
        "diff" | "view" => reply.files(),
        "template" => choices(
            "template",
            "next start vite react-router laravel astro",
            reply,
        ),
        "base" => choices("base", "base radix aria", reply),
        "client" => choices("MCP client", "claude cursor vscode codex opencode", reply),
        "only" | "type" => {
            let values = if flag.long() == Some("only") {
                "theme font"
            } else {
                "ui block hook component lib page file style theme item base font"
            };
            let (before, _) = prefix.rsplit_once(',').unwrap_or(("", prefix));
            let chosen: Vec<&str> = before.split(',').collect();
            if !before.is_empty() {
                reply.skip(format!("{leader}{before},"));
            }
            reply.group(
                Group::new("values", "part").suffix(",", true).items(
                    values
                        .split_whitespace()
                        .filter(|value| !chosen.contains(value))
                        .map(Item::bare),
                ),
            );
        }
        _ => shared::generic_value(flag, reply),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/shadcn_tests.rs"]
mod tests;
