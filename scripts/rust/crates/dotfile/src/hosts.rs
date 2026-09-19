use std::time::Duration;

use sysinfo::inventory::Host;

use crate::context::Context;

pub const FILE: &str = "config/hosts.dotfile";

/// This machine's entry in `config/hosts.dotfile`, when the file names it.
pub fn this(context: &Context) -> Option<String> {
    let source = std::fs::read_to_string(context.root.join(FILE)).ok()?;
    let hosts = sysinfo::inventory::parse_hosts(&source).ok()?;
    local(context, &hosts)
}

pub fn local(context: &Context, hosts: &[Host]) -> Option<String> {
    let inventory = context.inventory();
    let pinned = sysinfo::inventory::resolve_with(&inventory, hosts, "", &[]);
    let candidate = if pinned.is_empty() {
        sysinfo::inventory::resolve_with(&inventory, hosts, "", &local_hostnames(context))
    } else {
        pinned
    };
    hosts
        .iter()
        .find(|host| host.name == candidate)
        .map(|host| host.name.clone())
}

// Arch installs without inetutils have no `hostname` binary, and bash does not export HOSTNAME.
fn local_hostnames(context: &Context) -> Vec<String> {
    let hostname = context
        .env("HOSTNAME")
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty());
    let names = sysinfo::inventory::local_hostnames_with(hostname.as_deref(), |words| {
        let result = crate::process::output(
            context.command(words[0]).args(&words[1..]),
            hostkit::process::CaptureLimits::default(),
            Duration::from_secs(3),
        )
        .ok()?;
        (result.status.success() && !result.stdout_truncated)
            .then(|| String::from_utf8_lossy(&result.stdout).trim().to_string())
    });
    if names.is_empty() {
        sysinfo::inventory::local_hostnames()
    } else {
        names
    }
}
