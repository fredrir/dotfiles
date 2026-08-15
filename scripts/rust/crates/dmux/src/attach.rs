//! Transport selection, and the exec that hands the terminal over.
//!
//! Every attach ends in `CommandExt::exec`: dmux disappears and the terminal
//! belongs to tmux, ssh or `wezterm cli`. That is also why `DMUX_DRY_RUN`
//! exists — an exec cannot be observed from a test, so the dry run prints
//! the plan instead. Remote commands travel through the ssh host alias, so
//! ~/.ssh/config's cabled-first Match rules keep doing route selection for
//! everything tmux-shaped.

use std::io::{self, Write};
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

pub fn con(context: &Context, target: &str, window: Option<&str>) -> Result<ExitCode, String> {
    if let Some(window) = window {
        list::require_valid(window)?;
    }
    let rows = list::gather(context, true, true)?;
    let row = list::resolve(&rows, target)?.clone();
    attach_row(context, &row, window)
}

pub fn attach_row(context: &Context, row: &Row, window: Option<&str>) -> Result<ExitCode, String> {
    if row.kind == Kind::Wez {
        return Err(format!(
            "'{}' is a wezterm workspace; switch to it inside wezterm",
            row.name
        ));
    }
    record_departure(context, &row.name);
    Ok(exec_plan(plan_con(context, &row.name, window), &[]))
}

pub fn new_session(context: &Context, name: &str) -> Result<ExitCode, String> {
    list::require_valid(name)?;
    record_departure(context, name);
    Ok(exec_plan(plan_new(context, name), &[]))
}

pub fn toggle(context: &Context) -> Result<ExitCode, String> {
    let Some(previous) = state::previous(context.host) else {
        return Err(format!(
            "no previous session recorded for {}",
            context.host.name()
        ));
    };
    con(context, &previous, None)
}

pub fn remove(
    context: &Context,
    targets: &[String],
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
    for target in targets {
        let row = list::resolve(&rows, target)?;
        if row.kind == Kind::Wez {
            return Err(format!(
                "'{}' is a wezterm workspace; close it inside wezterm",
                row.name
            ));
        }
        chosen.push(row.clone());
    }
    if !yes && !confirmed(context, &chosen, window) {
        println!("{PROGRAM}: cancelled");
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

pub fn rename(context: &Context, old: &str, new: &str) -> Result<ExitCode, String> {
    list::require_valid(old)?;
    list::require_valid(new)?;
    let plan = if context.local {
        vec![
            "tmux".to_string(),
            "rename-session".to_string(),
            "-t".to_string(),
            format!("={old}"),
            new.to_string(),
        ]
    } else {
        ssh_run(context, format!("tmux rename-session -t ={old} {new}"))
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

fn plan_new(context: &Context, name: &str) -> Vec<String> {
    if !context.local {
        return ssh_attach(context, format!("exec tmux new-session -A -s {name}"));
    }
    if context.inside_tmux {
        let mut words = plan(&["tmux", "new-session", "-A", "-d", "-s"]);
        words.push(name.to_string());
        words.extend(plan(&[";", "switch-client", "-t"]));
        words.push(format!("={name}"));
        words
    } else {
        let mut words = plan(&["tmux", "new-session", "-A", "-s"]);
        words.push(name.to_string());
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

fn ssh_attach(context: &Context, command: String) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-t".to_string(),
        context.host.name().to_string(),
        command,
    ]
}

fn ssh_run(context: &Context, command: String) -> Vec<String> {
    vec!["ssh".to_string(), context.host.name().to_string(), command]
}

fn plan(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

/// A toggle target is only recorded when there is a session being left: the
/// one this process sits inside. Skipped on a dry run, which must not write.
fn record_departure(context: &Context, target: &str) {
    if dry_run() || !context.local || !context.inside_tmux {
        return;
    }
    let Ok(output) = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let current = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if current.is_empty() || current == target {
        return;
    }
    state::record(context.host, &current);
}

fn confirmed(context: &Context, rows: &[Row], window: Option<&str>) -> bool {
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    let what = match window {
        Some(window) => format!("window '{}:{window}'", names[0]),
        None if names.len() == 1 => format!("session '{}'", names[0]),
        None => format!("{} sessions ({})", names.len(), names.join(", ")),
    };
    ask(&format!("Kill {what} on {}? [y/N] ", context.host.name()))
}

/// Not `workstation::confirm`: that treats an empty answer as yes, and a
/// kill keeps the zsh version's [y/N] — only an explicit yes destroys.
fn ask(question: &str) -> bool {
    print!("{question}");
    if io::stdout().flush().is_err() {
        return false;
    }
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            println!();
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
/// braces and commas stay out of the plain set — `={a,b}` would expand.
fn quote(argument: &str) -> String {
    let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_-./=:@%+".contains(&byte);
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

    #[test]
    fn a_remote_attach_goes_over_ssh() {
        let plan = plan_con(&context(false, false, false), "main", None);
        assert_eq!(plan, ["ssh", "-t", "archie", "exec tmux attach -t =main"]);
    }

    #[test]
    fn a_remote_window_select_escapes_the_separator() {
        let plan = plan_con(&context(false, false, false), "main", Some("2"));
        assert_eq!(
            plan[3],
            "exec tmux attach -t =main \\; select-window -t =main:2"
        );
    }

    #[test]
    fn new_creates_and_attaches() {
        let plan = plan_new(&context(true, false, false), "scratch");
        assert_eq!(plan, ["tmux", "new-session", "-A", "-s", "scratch"]);
        let plan = plan_new(&context(false, false, false), "scratch");
        assert_eq!(
            plan,
            ["ssh", "-t", "archie", "exec tmux new-session -A -s scratch"]
        );
    }

    #[test]
    fn new_inside_tmux_detaches_then_switches() {
        let plan = plan_new(&context(true, false, true), "scratch");
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
    fn kills_use_exact_targets() {
        let plan = plan_kill(&context(true, false, false), "main");
        assert_eq!(plan, ["tmux", "kill-session", "-t", "=main"]);
        let plan = plan_kill_window(&context(false, false, false), "main", "2");
        assert_eq!(plan, ["ssh", "archie", "tmux kill-window -t =main:2"]);
    }

    #[test]
    fn quoting_survives_a_hostile_name() {
        assert_eq!(quote("=main"), "=main");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("$(reboot)"), "'$(reboot)'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("={a,b}"), "'={a,b}'");
        assert_eq!(
            shell_join(&plan(&["tmux", "attach", "-t", "=a", ";", "x"])),
            "tmux attach -t =a ';' x"
        );
    }
}
