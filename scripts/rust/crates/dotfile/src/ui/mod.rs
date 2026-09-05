pub mod plain;
pub mod tui;

use std::io::IsTerminal;
use std::path::Path;
use std::thread::JoinHandle;

use crossbeam_channel::Receiver;
use workstation::color::auto_enabled;
use workstation::path::home_relative;
use workstation::text::plural;

use crate::decision::{Choice, Prompt, Request, Server};
use crate::event::{Action, Event, Phase, Summary};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiPolicy {
    pub interactive: bool,
    pub color: bool,
    pub motion: bool,
}

impl UiPolicy {
    pub fn from_signals(
        stdin_is_terminal: bool,
        stderr_is_terminal: bool,
        term: Option<&str>,
        ci: Option<&str>,
        no_color: bool,
        reduced_motion: bool,
    ) -> Self {
        let capable_terminal = stdin_is_terminal
            && stderr_is_terminal
            && !term.is_some_and(|value| value.eq_ignore_ascii_case("dumb"))
            && !ci.is_some_and(environment_flag_enabled);
        let color = auto_enabled(capable_terminal, no_color, None, term);
        Self {
            interactive: capable_terminal,
            color,
            motion: color && !reduced_motion,
        }
    }

    fn detect() -> Self {
        let reduced_motion = [
            "DOTFILE_REDUCED_MOTION",
            "PREFERS_REDUCED_MOTION",
            "REDUCE_MOTION",
            "REDUCED_MOTION",
        ]
        .into_iter()
        .any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| environment_flag_enabled(&value))
        });
        Self::from_signals(
            std::io::stdin().is_terminal(),
            std::io::stderr().is_terminal(),
            std::env::var("TERM").ok().as_deref(),
            std::env::var("CI").ok().as_deref(),
            std::env::var_os("NO_COLOR").is_some()
                || std::env::var("CLICOLOR").ok().as_deref() == Some("0"),
            reduced_motion,
        )
    }
}

pub fn run(
    receiver: Receiver<Event>,
    decisions: Server,
    worker: JoinHandle<Result<Summary, String>>,
    verbose: bool,
) -> Result<Summary, String> {
    let policy = UiPolicy::detect();
    if policy.interactive {
        tui::run(receiver, decisions, worker, verbose, policy)
    } else {
        plain::run(receiver, decisions, worker, verbose)
    }
}

pub fn signal_exit_code() -> Option<u8> {
    let signal = tui_kit::termination_signal();
    (signal > 0).then_some((128 + signal).min(255) as u8)
}

pub fn completion_line(summary: &Summary) -> String {
    if summary.dry_run {
        let changes = if summary.changed == 0 {
            String::new()
        } else {
            format!(
                " {} {}",
                summary.changed,
                plural(summary.changed, "change", "changes")
            )
        };
        return match &summary.peer {
            Some(peer) => format!("○ Plan ready{changes} → {peer}"),
            None => format!("○ Plan ready{changes}"),
        };
    }
    if let Some(peer) = &summary.peer {
        let local = if summary.changed == 0 {
            String::new()
        } else if summary.remote_changed.is_some_and(|changed| changed > 0) {
            format!(
                " {} local {}",
                summary.changed,
                plural(summary.changed, "change", "changes")
            )
        } else {
            format!(
                " {} {}",
                summary.changed,
                plural(summary.changed, "change", "changes")
            )
        };
        let remote = match summary.remote_changed {
            Some(0) | None => String::new(),
            Some(changed) => format!(" {changed} {}", plural(changed, "change", "changes")),
        };
        return format!("✓ Synced{local} → {peer}{remote}");
    }
    match summary.changed {
        0 => "✓ Synced".to_string(),
        changed => format!(
            "✓ Synced {changed} {}",
            plural(changed, "change", "changes")
        ),
    }
}

pub(crate) fn finish_worker(
    worker: JoinHandle<Result<Summary, String>>,
    failure: Option<(String, Option<String>)>,
) -> Result<Summary, String> {
    let result = worker
        .join()
        .map_err(|_| "sync worker panicked".to_string())?;
    match result {
        Ok(mut summary) => {
            summary.profile = sanitize_text(&summary.profile);
            summary.peer = summary.peer.as_deref().map(sanitize_text);
            Ok(summary)
        }
        Err(error) => {
            let hint = failure.and_then(|(_, hint)| hint);
            let error = sanitize_text(&error);
            Err(match hint {
                Some(hint) => format!("{error}\n  hint: {}", sanitize_text(&hint)),
                None => error,
            })
        }
    }
}

pub(crate) fn settle_worker_after_ui_error(
    receiver: &Receiver<Event>,
    decisions: &Server,
    worker: JoinHandle<Result<Summary, String>>,
    pending: Option<Request>,
) {
    crate::cancel::request();
    if let Some(request) = pending {
        let _ = decisions.respond(&request, cancellation_choice(&request.prompt));
    }
    while !worker.is_finished() {
        while let Some(request) = decisions.try_recv() {
            let _ = decisions.respond(&request, cancellation_choice(&request.prompt));
        }
        while receiver.try_recv().is_ok() {}
        match receiver.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => std::thread::yield_now(),
        }
    }
    while receiver.try_recv().is_ok() {}
    let _ = worker.join();
}

pub(crate) fn cancellation_choice(prompt: &Prompt) -> Choice {
    match prompt {
        Prompt::Merge { .. } => Choice::Abort,
        Prompt::MergeTarget { .. } | Prompt::RemoteChanges { .. } => Choice::Cancel,
    }
}

pub(crate) fn action_name(action: Action) -> &'static str {
    match action {
        Action::Check => "check",
        Action::Create => "create",
        Action::Link => "link",
        Action::Prune => "prune",
        Action::Merge => "merge",
        Action::Secret => "secret",
        Action::Generate => "generate",
        Action::Push => "push",
        Action::Pull => "pull",
        Action::Sync => "sync",
    }
}

pub(crate) fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Preflight => "preflight",
        Phase::Tooling => "tooling",
        Phase::Artifacts => "metadata",
        Phase::Plan => "plan",
        Phase::Links => "links",
        Phase::Secrets => "secrets",
        Phase::Merge => "merge",
        Phase::Push => "push",
        Phase::Remote => "remote",
        Phase::Integrations => "integrations",
    }
}

pub(crate) fn item_line(action: Action, path: &Path, detail: &str) -> String {
    let path = compact_path(path);
    if detail.is_empty() {
        format!("{} {path}", action_name(action))
    } else {
        format!("{} {path} ({})", action_name(action), sanitize_text(detail))
    }
}

pub(crate) fn compact_path(target: &Path) -> String {
    sanitize_text(&home_relative(target))
}

pub(crate) fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            character if character.is_control() => '�',
            character => character,
        })
        .collect()
}

fn environment_flag_enabled(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
#[path = "../../tests/unit/ui/mod_tests.rs"]
mod tests;
