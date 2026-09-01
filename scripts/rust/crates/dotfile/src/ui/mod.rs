pub mod plain;
pub mod tui;

use std::io::IsTerminal;
use std::path::Path;
use std::thread::JoinHandle;

use crossbeam_channel::Receiver;

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
        Self {
            interactive: capable_terminal,
            color: capable_terminal && !no_color,
            motion: capable_terminal && !no_color && !reduced_motion,
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
    #[cfg(unix)]
    {
        let signal = tui::termination_signal();
        (signal > 0).then_some((128 + signal).min(255) as u8)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

pub fn completion_line(summary: &Summary) -> String {
    let unit = if summary.changed == 1 {
        "change"
    } else {
        "changes"
    };
    if let Some(peer) = &summary.peer {
        if summary.dry_run {
            return format!(
                "○ {} → {} · {} local {unit} pending",
                summary.profile, peer, summary.changed
            );
        }
        return match summary.remote_changed {
            Some(remote_changed) => format!(
                "✓ {} → {} synced · local {} · peer {} · {} checked · {} ms",
                summary.profile,
                peer,
                summary.changed,
                remote_changed,
                summary.checked,
                summary.elapsed.as_millis()
            ),
            None => format!(
                "✓ {} → {} synced · local {} · {} checked · {} ms",
                summary.profile,
                peer,
                summary.changed,
                summary.checked,
                summary.elapsed.as_millis()
            ),
        };
    }
    if summary.dry_run {
        format!("○ {} · {} {unit} pending", summary.profile, summary.changed)
    } else if summary.changed == 0 {
        format!(
            "✓ {} current · {} checked · {} ms",
            summary.profile,
            summary.checked,
            summary.elapsed.as_millis()
        )
    } else {
        format!(
            "✓ {} synced · {} changed · {} checked · {} ms",
            summary.profile,
            summary.changed,
            summary.checked,
            summary.elapsed.as_millis()
        )
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

pub(crate) fn compact_path(path: &Path) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return sanitize_text(&path.display().to_string());
    };
    let home = Path::new(&home);
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Ok(relative) => sanitize_text(&format!("~/{}", relative.display())),
        Err(_) => sanitize_text(&path.display().to_string()),
    }
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
mod tests {
    use super::*;

    #[test]
    fn sync_ui_text_never_contains_terminal_controls_or_extra_lines() {
        let sanitized = sanitize_text("safe\nforged\r\u{1b}[31m\u{7} alert\tend");
        assert_eq!(sanitized, "safe forged �[31m� alert end");
        assert!(!sanitized.chars().any(char::is_control));
    }
}
