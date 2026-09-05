use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    Result,
    config::{self, clean, identity},
    tmux::Context,
};

pub const MARKER_ON: &[u8] = b"\x1b]1337;SetUserVar=TMUX_WORKSPACE=MQ==\x07";
pub const MARKER_OFF: &[u8] = b"\x1b]1337;SetUserVar=TMUX_WORKSPACE=\x07";

pub fn marker(target: &str, enabled: bool) {
    if !target.starts_with("/dev/") {
        return;
    }
    if let Ok(mut file) = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW)
        .open(target)
        && let Ok(metadata) = file.metadata()
        && metadata.file_type().is_char_device()
        && metadata.uid() == nix::unistd::getuid().as_raw()
        && file.is_terminal()
    {
        let _ = file.write_all(if enabled { MARKER_ON } else { MARKER_OFF });
    }
}

pub fn update(ctx: &Context, remove: bool, tty: Option<&str>, origin: Option<&str>) -> Result<()> {
    let socket = ctx.tmux.socket()?;
    let _lock = config::lock(
        &ctx.paths
            .state
            .join("locks")
            .join(format!("{}-clients.lock", identity(&socket))),
        true,
    )?;
    let mut rows = ctx.tmux.clients()?;
    let client = ctx.client.as_deref().unwrap_or("");
    let internal = rows.iter().any(|c| c.name == client && c.internal);
    let target = rows
        .iter()
        .find(|c| c.name == client && !c.internal)
        .map(|c| c.tty.as_str())
        .or(tty)
        .or(remove.then_some(client));
    if let Some(target) = target
        && !internal
    {
        marker(target, !remove);
    }
    if remove {
        rows.retain(|c| c.name != client);
    }
    let mut label = "#{client_termname}".to_owned();
    let mut live = HashSet::new();
    for row in rows.iter().rev() {
        let key = format!("@workspace-client-{}-{}", row.pid, row.created);
        live.insert(key.clone());
        let mut stored = ctx.tmux.option(&key);
        if let Some(origin) = origin
            && row.name == client
        {
            stored = format!("{} → {}", clean(origin, 60), config::hostname())
                .chars()
                .filter(|c| c.is_alphanumeric() || " .@:/→+-".contains(*c))
                .collect();
            ctx.tmux.set(&key, &stored)?;
        }
        if stored.is_empty() {
            stored = clean(&row.term, 50);
        }
        let condition = format!(
            "#{{&&:#{{==:#{{client_pid}},{}}},#{{==:#{{client_created}},{}}}}}",
            row.pid, row.created
        );
        label = format!(
            "#{{?{condition},{},{label}}}",
            stored.replace('#', "##").replace(',', "")
        );
    }
    ctx.tmux.set("@workspace-client-label", &label)?;
    let pattern = regex::Regex::new(r"^@workspace-client-\d+-\d+$")?;
    for line in ctx.tmux.run(&["show-options", "-g"])?.lines() {
        let key = line.split_whitespace().next().unwrap_or("");
        if pattern.is_match(key) && !live.contains(key) {
            ctx.tmux.run(&["set-option", "-gu", key])?;
        }
    }
    Ok(())
}

struct Marker;
impl Drop for Marker {
    fn drop(&mut self) {
        if std::io::stdout().is_terminal() {
            let _ = std::io::stdout().write_all(MARKER_OFF);
        }
    }
}

pub fn attach(ctx: &mut Context, id: &str, origin: Option<&str>) -> Result<i32> {
    let mut child = ctx
        .tmux
        .command()
        .args(["attach-session", "-t", id])
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .spawn()?;
    let _marker = Marker;
    if std::io::stdout().is_terminal() {
        let _ = std::io::stdout().write_all(MARKER_ON);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut registered = false;
    while child.try_wait()?.is_none() && Instant::now() < deadline {
        if let Some(row) = ctx
            .tmux
            .clients()?
            .into_iter()
            .find(|c| c.pid == child.id().to_string())
        {
            ctx.client = Some(row.name);
            update(ctx, false, None, origin)?;
            registered = true;
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    let status = child.wait()?;
    if registered {
        let _ = update(ctx, true, None, None);
    }
    Ok(status.code().unwrap_or(1))
}
