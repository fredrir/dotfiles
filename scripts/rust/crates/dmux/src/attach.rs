//! Transport selection, and the exec that hands the terminal over.
//!
//! Every attach ends in `CommandExt::exec`: dmux disappears and the terminal
//! belongs to tmux, ssh or `wezterm cli`. That is also why `DMUX_DRY_RUN`
//! exists — an exec cannot be observed from a test, so the dry run prints
//! the plan instead. Remote commands travel through the ssh host alias, so
//! ~/.ssh/config's cabled-first Match rules keep doing route selection for
//! everything tmux-shaped.

use std::io::{self, IsTerminal, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use crate::PROGRAM;
use crate::hosts::{self, Context};
use crate::list::{self, Kind, Row};
use crate::state;

pub fn dry_run() -> bool {
    std::env::var_os("DMUX_DRY_RUN").is_some_and(|value| !value.is_empty() && value != "0")
}

pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// The bare attach the old ssa/ssm defaulted to: native wezterm domain when
/// inside wezterm — over the cable when it answers, Tailscale otherwise —
/// and tmux session `main` everywhere else.
pub fn bare(context: &Context) -> Result<ExitCode, String> {
    if context.inside_wezterm {
        if context.local {
            return Ok(exec_plan(plan(&["wezterm", "cli", "spawn"]), &[]));
        }
        let peer = context.host.name();
        let domain = if hosts::usb_latency(hosts::PROBE_TIMEOUT).is_some() {
            format!("{peer}-usb")
        } else {
            eprintln!("{PROGRAM}: usb link down, attaching {peer}-ts");
            format!("{peer}-ts")
        };
        let mut spawn = plan(&["wezterm", "cli", "spawn", "--domain-name"]);
        spawn.push(domain);
        return Ok(exec_plan(spawn, &[]));
    }
    new_session(context, "main")
}

pub fn con(
    context: &Context,
    target: &str,
    window: Option<&str>,
    create: bool,
) -> Result<ExitCode, String> {
    if let Some(window) = window {
        list::require_valid(window)?;
    }
    let rows = list::gather(context, true, true)?;
    let row = match list::resolve(&rows, target) {
        Ok(row) => row.clone(),
        // -A: the old ssa default — fall back to what `dmux new` would do,
        // name validation included.
        Err(_) if create => return new_session(context, target),
        Err(error) => return Err(error),
    };
    attach_row(context, &row, window)
}

pub fn attach_row(context: &Context, row: &Row, window: Option<&str>) -> Result<ExitCode, String> {
    if row.kind == Kind::Wez {
        return attach_wez(context, row, window);
    }
    record_departure(context, &row.name);
    Ok(exec_plan(plan_con(context, &row.name, window), &[]))
}

/// A wezterm workspace has no attach protocol, but inside wezterm (per the
/// same trust rule everything else uses) activating one of its panes makes
/// the GUI switch to it. Outside wezterm there is nothing that could switch,
/// so the honest error stands. No toggle state is recorded: `dmux -` moves
/// between tmux sessions, and a workspace switch is not a departure.
fn attach_wez(context: &Context, row: &Row, window: Option<&str>) -> Result<ExitCode, String> {
    if !context.inside_wezterm {
        return Err(format!(
            "'{}' is a wezterm workspace; switch to it inside wezterm",
            row.name
        ));
    }
    if window.is_some() {
        return Err(format!(
            "-w selects tmux windows; '{}' is a wezterm workspace",
            row.name
        ));
    }
    let Some(pane) = row.pane else {
        return Err(format!("'{}' lists no pane to activate", row.name));
    };
    let mut plan = plan(&["wezterm", "cli", "activate-pane", "--pane-id"]);
    plan.push(pane.to_string());
    Ok(exec_plan(plan, &[]))
}

pub fn new_session(context: &Context, name: &str) -> Result<ExitCode, String> {
    new_session_in(context, name, None, &[])
}

/// `--dir` and a trailing command ride the same plan. tmux's own `-A`
/// semantics apply: when the session already exists both are ignored and the
/// attach simply happens.
pub fn new_session_in(
    context: &Context,
    name: &str,
    dir: Option<&str>,
    command: &[String],
) -> Result<ExitCode, String> {
    list::require_valid(name)?;
    record_departure(context, name);
    Ok(exec_plan(plan_new(context, name, dir, command), &[]))
}

/// Detach is a tmux concept — the client leaves, the session keeps running.
/// Wezterm has no equivalent: a GUI window is not attached the way a tmux
/// client is, so anywhere but inside tmux the honest answer is an error.
pub fn detach(context: &Context) -> Result<ExitCode, String> {
    if context.inside_tmux {
        return Ok(exec_plan(plan(&["tmux", "detach-client"]), &[]));
    }
    if context.inside_wezterm {
        return Err(
            "wezterm windows do not detach; detach applies inside a tmux session".to_string(),
        );
    }
    Err("not inside a tmux session; nothing to detach".to_string())
}

pub fn toggle(context: &Context) -> Result<ExitCode, String> {
    let Some(previous) = state::previous(context.host) else {
        return Err(format!(
            "no previous session recorded for {}",
            context.host.name()
        ));
    };
    con(context, &previous, None, false)
}

pub fn remove(
    context: &Context,
    targets: &[String],
    all: bool,
    window: Option<&str>,
    yes: bool,
) -> Result<ExitCode, String> {
    if let Some(window) = window {
        list::require_valid(window)?;
        if targets.len() != 1 {
            return Err("-w takes exactly one session".to_string());
        }
    }
    let rows = list::gather(context, true, true)?;
    let mut chosen = Vec::new();
    if all {
        chosen = chosen_for_all(context, &rows);
        if chosen.is_empty() {
            eprintln!("{PROGRAM}: nothing to kill on {}", context.host.name());
            return Ok(ExitCode::SUCCESS);
        }
    }
    for target in targets {
        let row = list::resolve(&rows, target)?;
        note_index_target(&rows, target, row);
        if row.kind == Kind::Wez {
            return Err(format!(
                "'{}' is a wezterm workspace; close it inside wezterm",
                row.name
            ));
        }
        chosen.push(row.clone());
    }
    if !yes && !confirmed(context, &chosen, window)? {
        eprintln!("{PROGRAM}: cancelled");
        return Ok(ExitCode::SUCCESS);
    }
    let mut all_ok = true;
    for row in &chosen {
        let plan = match window {
            Some(window) => plan_kill_window(context, &row.name, window),
            None => plan_kill(context, &row.name),
        };
        all_ok &= run_plan(plan)?;
    }
    Ok(if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// `--all` is every tmux session in the listing. Wezterm workspaces are not
/// killable over this path and are left alone with a note, and the session
/// this client sits inside is kept — killing it would take the terminal down
/// mid-sweep — with a note saying how to kill it explicitly.
fn chosen_for_all(context: &Context, rows: &[Row]) -> Vec<Row> {
    let workspaces = rows.iter().filter(|row| row.kind == Kind::Wez).count();
    if workspaces > 0 {
        eprintln!(
            "{PROGRAM}: leaving {workspaces} wezterm workspace(s) alone; close them inside wezterm"
        );
    }
    let current = current_session(context);
    let mut chosen = Vec::new();
    for row in rows.iter().filter(|row| row.kind == Kind::Tmux) {
        if current.as_deref() == Some(row.name.as_str()) {
            eprintln!(
                "{PROGRAM}: keeping current session '{}' (dmux rm {} to kill it)",
                row.name, row.name
            );
            continue;
        }
        chosen.push(row.clone());
    }
    chosen
}

/// A bare number is a transient row index here and a permanent Space number
/// once the Wez-first gate is on, so a target that was consumed as an index
/// says so on stderr before the two spellings swap meaning (plan §17.13,
/// case 44). It lives in the callers rather than in `list::resolve`, whose
/// resolution rule — and the unit tests pinning it — must not move.
///
/// The replacement it offers is the NAME, never `--row N`: this listing is
/// wez rows then tmux rows (`list::gather`), while the gated one is managed
/// rows by permanent SpaceNo then unmanaged (`inventory::reconcile`), so the
/// same N routinely names a different resource on the other side of the
/// gate. Handing an operator that substitution on a destructive verb is the
/// silent retarget case 44 exists to prevent.
fn note_index_target(rows: &[Row], target: &str, chosen: &Row) {
    if target.is_empty()
        || !target.bytes().all(|byte| byte.is_ascii_digit())
        || rows.iter().any(|row| row.name == target)
    {
        return;
    }
    eprintln!(
        "{PROGRAM}: '{target}' matched listing row {target} ('{}'), not a name; row indices go \
         away next release — use the name '{}'",
        chosen.name, chosen.name
    );
}

/// The old target resolves like con/rm's — index or exact name, exact name
/// winning — so a session another tool created with a nonconforming name
/// (spaces and all) can still be renamed. Only the new name must conform.
pub fn rename(context: &Context, old: &str, new: &str) -> Result<ExitCode, String> {
    list::require_valid(new)?;
    let rows = list::gather(context, true, true)?;
    let row = list::resolve(&rows, old)?;
    note_index_target(&rows, old, row);
    if row.kind == Kind::Wez {
        return Err(format!(
            "'{}' is a wezterm workspace; rename it inside wezterm",
            row.name
        ));
    }
    let old = row.name.as_str();
    let plan = if context.local {
        vec![
            "tmux".to_string(),
            "rename-session".to_string(),
            "-t".to_string(),
            format!("={old}"),
            new.to_string(),
        ]
    } else {
        ssh_run(
            context,
            format!(
                "tmux rename-session -t {} {}",
                quote(&format!("={old}")),
                quote(new)
            ),
        )
    };
    Ok(if run_plan(plan)? {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The `=` prefix makes every tmux target an exact name, never a prefix
/// match. Inside tmux the client switches instead of attaching, since tmux
/// refuses a nested attach.
fn plan_con(context: &Context, name: &str, window: Option<&str>) -> Vec<String> {
    if context.local {
        let verb = if context.inside_tmux {
            "switch-client"
        } else {
            "attach"
        };
        let mut plan = plan(&["tmux", verb, "-t"]);
        plan.push(format!("={name}"));
        if let Some(window) = window {
            plan.extend(plan_select_window(name, window));
        }
        return plan;
    }
    let mut command = format!("exec tmux attach -t {}", quote(&format!("={name}")));
    if let Some(window) = window {
        command.push_str(&format!(
            " \\; select-window -t {}",
            quote(&format!("={name}:{window}"))
        ));
    }
    ssh_attach(context, command)
}

fn plan_new(context: &Context, name: &str, dir: Option<&str>, command: &[String]) -> Vec<String> {
    if !context.local {
        let mut remote = format!("exec tmux new-session -A -s {name}");
        if let Some(dir) = dir {
            remote.push_str(&format!(" -c {}", quote(dir)));
        }
        for word in command {
            remote.push_str(&format!(" {}", quote(word)));
        }
        return ssh_attach(context, remote);
    }
    if context.inside_tmux {
        let mut words = plan(&["tmux", "new-session", "-A", "-d", "-s"]);
        words.push(name.to_string());
        if let Some(dir) = dir {
            words.push("-c".to_string());
            words.push(dir.to_string());
        }
        words.extend(command.iter().cloned());
        words.extend(plan(&[";", "switch-client", "-t"]));
        words.push(format!("={name}"));
        words
    } else {
        let mut words = plan(&["tmux", "new-session", "-A", "-s"]);
        words.push(name.to_string());
        if let Some(dir) = dir {
            words.push("-c".to_string());
            words.push(dir.to_string());
        }
        words.extend(command.iter().cloned());
        words
    }
}

fn plan_kill(context: &Context, name: &str) -> Vec<String> {
    if context.local {
        let mut plan = plan(&["tmux", "kill-session", "-t"]);
        plan.push(format!("={name}"));
        plan
    } else {
        ssh_run(
            context,
            format!("tmux kill-session -t {}", quote(&format!("={name}"))),
        )
    }
}

fn plan_kill_window(context: &Context, name: &str, window: &str) -> Vec<String> {
    if context.local {
        let mut plan = plan(&["tmux", "kill-window", "-t"]);
        plan.push(format!("={name}:{window}"));
        plan
    } else {
        ssh_run(
            context,
            format!(
                "tmux kill-window -t {}",
                quote(&format!("={name}:{window}"))
            ),
        )
    }
}

fn plan_select_window(name: &str, window: &str) -> Vec<String> {
    let mut plan = plan(&[";", "select-window", "-t"]);
    plan.push(format!("={name}:{window}"));
    plan
}

/// Both ssh transports prepend `REMOTE_PATH_PREFIX` here, the one funnel
/// every remote command passes through, so no call site can forget that
/// macOS sshd's non-interactive PATH lacks tmux.
fn ssh_attach(context: &Context, command: String) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        hosts::SSH_CONNECT_TIMEOUT.to_string(),
        "-t".to_string(),
        context.host.name().to_string(),
        format!("{}{command}", hosts::REMOTE_PATH_PREFIX),
    ]
}

fn ssh_run(context: &Context, command: String) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        hosts::SSH_CONNECT_TIMEOUT.to_string(),
        context.host.name().to_string(),
        format!("{}{command}", hosts::REMOTE_PATH_PREFIX),
    ]
}

fn plan(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

/// A toggle target is only recorded when there is a session being left. On
/// this machine that is the session this process sits inside; a peer cannot
/// be asked, so remote attaches instead track the last session dmux attached
/// there (see `state::record_attach`). Skipped on a dry run, which must not
/// write.
fn record_departure(context: &Context, target: &str) {
    if dry_run() {
        return;
    }
    if !context.local {
        state::record_attach(context.host, target);
        return;
    }
    let Some(current) = current_session(context) else {
        return;
    };
    if current != target {
        state::record(context.host, &current);
    }
}

/// The tmux session this process sits inside, when there is one to ask.
/// Read-only, so it runs on a dry run too — which is what lets `rm --all`'s
/// keep-the-current-session rule be tested through the stub tmux.
fn current_session(context: &Context) -> Option<String> {
    if !context.local || !context.inside_tmux {
        return None;
    }
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let current = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!current.is_empty()).then_some(current)
}

/// A confirmation needs someone to ask: without a terminal on stdin a real
/// kill refuses loudly instead of blocking on a pipe that may never answer
/// (or reading EOF and silently doing nothing). A dry run destroys nothing,
/// so it keeps reading whatever stdin is — that is also what lets the
/// prompt logic itself be tested through a pipe.
fn confirmed(context: &Context, rows: &[Row], window: Option<&str>) -> Result<bool, String> {
    if !io::stdin().is_terminal() && !dry_run() {
        return Err("stdin is not a terminal; pass --yes to kill without confirmation".to_string());
    }
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    let what = match window {
        Some(window) => format!("window '{}:{window}'", names[0]),
        None if names.len() == 1 => format!("session '{}'", names[0]),
        None => format!("{} sessions ({})", names.len(), names.join(", ")),
    };
    Ok(ask(&format!(
        "Kill {what} on {}? [y/N] ",
        context.host.name()
    )))
}

/// Not `workstation::confirm`: that treats an empty answer as yes, and a
/// kill keeps the zsh version's [y/N] — only an explicit yes destroys. The
/// prompt goes to stderr so it never contaminates captured output.
fn ask(question: &str) -> bool {
    eprint!("{question}");
    if io::stderr().flush().is_err() {
        return false;
    }
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            eprintln!();
            false
        }
        Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
    }
}

pub fn exec_plan(plan: Vec<String>, envs: &[(&str, String)]) -> ExitCode {
    if dry_run() {
        println!("would exec: {}", shell_join(&plan));
        return ExitCode::SUCCESS;
    }
    let mut command = Command::new(&plan[0]);
    command.args(&plan[1..]);
    for (name, value) in envs {
        command.env(name, value);
    }
    let error = command.exec();
    workstation::fail(PROGRAM, format!("{}: {error}", plan[0]))
}

fn run_plan(plan: Vec<String>) -> Result<bool, String> {
    if dry_run() {
        println!("would run: {}", shell_join(&plan));
        return Ok(true);
    }
    let status = Command::new(&plan[0])
        .args(&plan[1..])
        .status()
        .map_err(|error| format!("{}: {error}", plan[0]))?;
    Ok(status.success())
}

fn shell_join(plan: &[String]) -> String {
    plan.iter()
        .map(|argument| quote(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Enough quoting for two readers: a human checking a dry run, and a remote
/// shell receiving the ssh command string. The remote login shell is zsh, so
/// braces and commas stay out of the plain set — `={a,b}` would expand — and
/// so does `=`: with the EQUALS option, zsh rewrites an unquoted `=word`
/// into a PATH lookup, so a bare `=main` target dies with "main not found"
/// (or silently becomes a filesystem path when the name shadows a command).
fn quote(argument: &str) -> String {
    let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_-./:@%+".contains(&byte);
    if !argument.is_empty() && argument.bytes().all(plain) {
        return argument.to_string();
    }
    format!("'{}'", argument.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::Host;

    fn context(local: bool, inside_wezterm: bool, inside_tmux: bool) -> Context {
        Context {
            host: if local { Host::Macie } else { Host::Archie },
            local,
            inside_wezterm,
            inside_tmux,
        }
    }

    #[test]
    fn a_local_attach_is_plain_tmux() {
        let plan = plan_con(&context(true, false, false), "main", None);
        assert_eq!(plan, ["tmux", "attach", "-t", "=main"]);
    }

    #[test]
    fn inside_tmux_the_client_switches() {
        let plan = plan_con(&context(true, false, true), "main", None);
        assert_eq!(plan, ["tmux", "switch-client", "-t", "=main"]);
    }

    #[test]
    fn a_window_is_selected_after_the_attach() {
        let plan = plan_con(&context(true, false, false), "main", Some("2"));
        assert_eq!(
            plan,
            [
                "tmux",
                "attach",
                "-t",
                "=main",
                ";",
                "select-window",
                "-t",
                "=main:2"
            ]
        );
    }

    /// Every remote command string opens with the PATH prefix — macOS sshd's
    /// non-interactive PATH lacks Homebrew's tmux — and the target rides
    /// after it, quoted as before.
    #[test]
    fn a_remote_attach_quotes_the_equals_target() {
        let plan = plan_con(&context(false, false, false), "main", None);
        let command = format!("{}exec tmux attach -t '=main'", hosts::REMOTE_PATH_PREFIX);
        assert_eq!(
            plan,
            [
                "ssh",
                "-o",
                "ConnectTimeout=5",
                "-t",
                "archie",
                command.as_str()
            ]
        );
    }

    #[test]
    fn a_remote_window_select_escapes_the_separator() {
        let plan = plan_con(&context(false, false, false), "main", Some("2"));
        assert_eq!(
            plan.last().unwrap(),
            &format!(
                "{}exec tmux attach -t '=main' \\; select-window -t '=main:2'",
                hosts::REMOTE_PATH_PREFIX
            )
        );
    }

    #[test]
    fn new_creates_and_attaches() {
        let plan = plan_new(&context(true, false, false), "scratch", None, &[]);
        assert_eq!(plan, ["tmux", "new-session", "-A", "-s", "scratch"]);
        let plan = plan_new(&context(false, false, false), "scratch", None, &[]);
        let command = format!(
            "{}exec tmux new-session -A -s scratch",
            hosts::REMOTE_PATH_PREFIX
        );
        assert_eq!(
            plan,
            [
                "ssh",
                "-o",
                "ConnectTimeout=5",
                "-t",
                "archie",
                command.as_str()
            ]
        );
    }

    #[test]
    fn new_inside_tmux_detaches_then_switches() {
        let plan = plan_new(&context(true, false, true), "scratch", None, &[]);
        assert_eq!(
            plan,
            [
                "tmux",
                "new-session",
                "-A",
                "-d",
                "-s",
                "scratch",
                ";",
                "switch-client",
                "-t",
                "=scratch"
            ]
        );
    }

    #[test]
    fn new_carries_the_directory_and_command() {
        let command = ["nvim".to_string(), ".".to_string()];
        let plan = plan_new(&context(true, false, false), "s", Some("/tmp/x"), &command);
        assert_eq!(
            plan,
            [
                "tmux",
                "new-session",
                "-A",
                "-s",
                "s",
                "-c",
                "/tmp/x",
                "nvim",
                "."
            ]
        );
        let plan = plan_new(&context(true, false, true), "s", Some("/tmp/x"), &command);
        assert_eq!(
            plan,
            [
                "tmux",
                "new-session",
                "-A",
                "-d",
                "-s",
                "s",
                "-c",
                "/tmp/x",
                "nvim",
                ".",
                ";",
                "switch-client",
                "-t",
                "=s"
            ]
        );
    }

    /// The remote plan quotes the directory and every command word: a space
    /// or a hostile character must reach the peer's tmux intact.
    #[test]
    fn a_remote_new_quotes_directory_and_command() {
        let command = ["echo".to_string(), "hi there".to_string()];
        let plan = plan_new(
            &context(false, false, false),
            "s",
            Some("/tmp/a b"),
            &command,
        );
        assert_eq!(
            plan.last().unwrap(),
            &format!(
                "{}exec tmux new-session -A -s s -c '/tmp/a b' echo 'hi there'",
                hosts::REMOTE_PATH_PREFIX
            )
        );
    }

    #[test]
    fn kills_use_exact_targets() {
        let plan = plan_kill(&context(true, false, false), "main");
        assert_eq!(plan, ["tmux", "kill-session", "-t", "=main"]);
        let plan = plan_kill_window(&context(false, false, false), "main", "2");
        let command = format!("{}tmux kill-window -t '=main:2'", hosts::REMOTE_PATH_PREFIX);
        assert_eq!(
            plan,
            ["ssh", "-o", "ConnectTimeout=5", "archie", command.as_str()]
        );
    }

    #[test]
    fn quoting_survives_a_hostile_name() {
        // `=main` must be quoted: zsh's EQUALS option expands a bare =word.
        assert_eq!(quote("=main"), "'=main'");
        assert_eq!(quote("main"), "main");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("$(reboot)"), "'$(reboot)'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("={a,b}"), "'={a,b}'");
        assert_eq!(
            shell_join(&plan(&["tmux", "attach", "-t", "=a", ";", "x"])),
            "tmux attach -t '=a' ';' x"
        );
    }
}
