pub mod manager;
pub mod npmrc;
pub mod popular;
pub mod project;
pub mod registry;

use crate::context::Context;
use crate::help::{Flag, Help};
use crate::line::{Line, scan};
use crate::reply::{Group, Item, Reply, Tone};
use crate::shared;
use manager::{Manager, Role, RoleSpec, Spec};
use npmrc::Npmrc;

const WORKSPACE_FLAGS: &[&str] = &["workspace", "filter"];

pub fn complete(ctx: &Context, line: &Line, manager: Manager, runner: bool) -> Reply {
    let spec = manager::spec(ctx, manager);
    let mut reply = Reply::new();
    if runner {
        let role = spec.role(Role::Dlx);
        let help = role.map(|role| &role.help);
        role_arguments(
            ctx,
            line,
            manager,
            &spec,
            Role::Dlx,
            help,
            line.before(),
            &mut reply,
        );
        return reply;
    }

    let before = line.before();
    let scanned = scan(before, &[&spec.top]);
    if let Some(flag) = scanned.pending {
        flag_value(ctx, flag, &line.prefix, &mut reply);
        return reply;
    }
    let mut positionals = scanned.positionals.iter().copied();
    let Some(mut subcommand) = positionals.next() else {
        top_level(ctx, line, &spec, &mut reply);
        return reply;
    };
    let yarn_global = manager == Manager::Yarn && subcommand == "global";
    if yarn_global {
        match positionals.next() {
            Some(next) => subcommand = next,
            None => {
                reply.group(
                    Group::new("commands", "yarn global command")
                        .items(["add", "remove", "upgrade", "list", "bin"].map(Item::bare)),
                );
                return reply;
            }
        }
    }
    let Some(role) = spec.role_of(subcommand) else {
        reply.delegate();
        reply.files();
        return reply;
    };
    let start = before
        .iter()
        .position(|word| word == subcommand)
        .map_or(before.len(), |at| at + 1);
    let global = yarn_global || line.has_flag(&["-g", "--global"]);
    let context = RoleContext { global };
    role_arguments_with(
        ctx,
        line,
        manager,
        &spec,
        role,
        &before[start..],
        context,
        &mut reply,
    );
    reply
}

#[derive(Clone, Copy, Default)]
struct RoleContext {
    global: bool,
}

#[allow(clippy::too_many_arguments)]
fn role_arguments(
    ctx: &Context,
    line: &Line,
    manager: Manager,
    spec: &Spec,
    role: Role,
    help: Option<&Help>,
    words: &[String],
    reply: &mut Reply,
) {
    let fallback = RoleSpec {
        role,
        command: String::new(),
        aliases: Vec::new(),
        help: help.cloned().unwrap_or_default(),
    };
    role_arguments_with(
        ctx,
        line,
        manager,
        spec,
        &fallback,
        words,
        RoleContext::default(),
        reply,
    );
}

#[allow(clippy::too_many_arguments)]
fn role_arguments_with(
    ctx: &Context,
    line: &Line,
    manager: Manager,
    spec: &Spec,
    role: &RoleSpec,
    words: &[String],
    context: RoleContext,
    reply: &mut Reply,
) {
    let scanned = scan(words, &[&role.help, &spec.top]);
    if let Some(flag) = scanned.pending {
        flag_value(ctx, flag, &line.prefix, reply);
        return;
    }
    let prefix = line.prefix.as_str();
    if prefix.starts_with('-') && !scanned.after_separator {
        if let Some((name, value)) = prefix.split_once('=') {
            let named = Flag {
                names: vec![name.to_string()],
                ..Flag::default()
            };
            let flag = role
                .help
                .flag(name)
                .or_else(|| spec.top.flag(name))
                .unwrap_or(&named);
            reply.skip(format!("{name}="));
            flag_value(ctx, flag, value, reply);
            return;
        }
        shared::flags(&role.help, reply);
        return;
    }
    match role.role {
        Role::Add => packages(ctx, prefix, context.global, reply),
        Role::Remove | Role::Update => installed(ctx, manager, prefix, context.global, reply),
        Role::Run if scanned.positionals.is_empty() => scripts(ctx, reply),
        Role::Dlx if scanned.positionals.is_empty() => {
            let binaries = project::local_binaries(ctx);
            reply.group(
                Group::new("binaries", "local binary").items(
                    binaries
                        .into_iter()
                        .map(|name| Item::new(name, "node_modules/.bin")),
                ),
            );
            packages(ctx, prefix, true, reply);
        }
        Role::Run | Role::Dlx => reply.files(),
    }
}

fn top_level(ctx: &Context, line: &Line, spec: &Spec, reply: &mut Reply) {
    reply.delegate();
    if line.prefix.starts_with('-') {
        shared::flags(&spec.top, reply);
        return;
    }
    reply.group(
        Group::new("commands", "command").items(
            spec.top
                .commands
                .iter()
                .map(|command| Item::new(&command.name, &command.description)),
        ),
    );
    scripts(ctx, reply);
}

fn flag_value(ctx: &Context, flag: &Flag, prefix: &str, reply: &mut Reply) {
    let long = flag.long().unwrap_or("");
    if WORKSPACE_FLAGS.contains(&long) {
        let workspaces = project::workspaces(ctx);
        reply.group(
            Group::new("workspaces", "workspace").items(
                workspaces
                    .into_iter()
                    .map(|workspace| Item::new(workspace.name, workspace.path)),
            ),
        );
        if prefix.starts_with(['.', '/']) {
            reply.directories();
        }
        return;
    }
    shared::generic_value(flag, reply);
}

// A bare word lists popular packages: commands for global installs and runners, libraries otherwise.
fn packages(ctx: &Context, prefix: &str, commands: bool, reply: &mut Reply) {
    if prefix.starts_with(['.', '/', '~']) || prefix.starts_with("file:") {
        reply.files();
        return;
    }
    let npmrc = Npmrc::load(ctx);
    if let Some((name, _)) = registry::split_version(prefix) {
        versions(
            ctx,
            &npmrc.registry_for(name),
            name,
            &format!("{name}@"),
            reply,
        );
        return;
    }
    if prefix.is_empty() {
        popular_packages(ctx, commands, reply);
        return;
    }
    let hits = registry::search(ctx, &npmrc.registry_for(prefix), prefix);
    reply.group(found(Group::new("registry", "registry package"), hits));
}

fn popular_packages(ctx: &Context, commands: bool, reply: &mut Reply) {
    let Some(popular) = popular::load(ctx) else {
        return;
    };
    let (label, hits) = if commands {
        ("popular command", popular.tools)
    } else {
        ("popular package", popular.libraries)
    };
    reply.group(found(Group::new("popular", label), hits));
}

// Search hits replace the typed word, so names that only contain it are offered too.
pub fn found(group: Group, hits: Vec<registry::Hit>) -> Group {
    group
        .unsorted()
        .replace()
        .items(hits.into_iter().map(|hit| {
            let downloads = (hit.downloads > 0).then(|| format!("{}/mo", compact(hit.downloads)));
            Item::bare(hit.name)
                .detail(Tone::Info, downloads.unwrap_or_default())
                .detail(Tone::Muted, hit.description)
        }))
}

pub fn compact(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}K", count / 1_000),
        1_000_000..1_000_000_000 => format!("{}M", count / 1_000_000),
        _ => format!("{}B", count / 1_000_000_000),
    }
}

// Versions and dist-tags of `name`, completed after `skip`.
pub fn versions(ctx: &Context, registry: &str, name: &str, skip: &str, reply: &mut Reply) {
    reply.skip(skip);
    let Some(versions) = registry::versions(ctx, registry, name) else {
        reply.message(format!("no versions found for {name}"));
        return;
    };
    reply.group(
        Group::new("dist-tags", "dist-tag")
            .tone(Tone::Success)
            .unsorted()
            .items(
                versions
                    .tags
                    .iter()
                    .map(|(tag, version)| Item::new(tag, version)),
            ),
    );
    let tagged = |version: &str| {
        versions
            .tags
            .iter()
            .filter(|(_, tagged)| tagged == version)
            .map(|(tag, _)| tag.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    reply.group(
        Group::new("versions", "version").unsorted().items(
            versions
                .versions
                .iter()
                .map(|version| Item::bare(version).detail(Tone::Success, tagged(version))),
        ),
    );
}

fn installed(ctx: &Context, manager: Manager, prefix: &str, global: bool, reply: &mut Reply) {
    let dependencies = if global {
        project::global_packages(ctx, manager)
    } else {
        project::nearest(ctx)
            .map(|manifest| manifest.dependencies())
            .unwrap_or_default()
    };
    reply.group(
        Group::new(
            "dependencies",
            if global {
                "global package"
            } else {
                "dependency"
            },
        )
        .items(
            dependencies
                .into_iter()
                .filter(|dependency| dependency.name.starts_with(prefix))
                .map(describe_dependency),
        ),
    );
}

fn describe_dependency(dependency: project::Dependency) -> Item {
    let kind = match dependency.kind {
        "global" => "",
        kind => kind,
    };
    Item::new(dependency.name, dependency.range).detail(Tone::Warning, kind)
}

fn scripts(ctx: &Context, reply: &mut Reply) {
    let Some(manifest) = project::nearest(ctx) else {
        return;
    };
    reply.group(
        Group::new("scripts", "script").unsorted().items(
            manifest
                .scripts()
                .into_iter()
                .map(|(name, command)| Item::new(name, command)),
        ),
    );
}

#[cfg(test)]
#[path = "../../tests/unit/node/mod_tests.rs"]
mod tests;
