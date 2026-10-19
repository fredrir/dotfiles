use super::*;
use std::io::{Read, Seek};

fn captured_output(mut output: std::fs::File) -> Result<String, String> {
    output.rewind().map_err(|error| error.to_string())?;
    let limit = 256 * 1024;
    let length = output.metadata().map_err(|error| error.to_string())?.len();
    if length > limit {
        output
            .seek(std::io::SeekFrom::End(-(limit as i64)))
            .map_err(|error| error.to_string())?;
    }
    let mut bytes = Vec::new();
    output
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    Ok(if length > limit {
        format!("[Earlier output omitted; showing the last 256 KiB]\n{text}")
    } else {
        text
    })
}

fn execute_commit(repo: &Repo, spec: &CommandSpec) -> Result<String, String> {
    let mut lookup = repo.command(&CommandSpec::new("Editor", ["var", "GIT_EDITOR"]));
    let configured = hostkit::process::output(
        &mut lookup,
        hostkit::process::CaptureLimits::default(),
        Duration::from_secs(5),
    )
    .map_err(|error| error.to_string())?;
    if !configured.status.success() {
        return Err(String::from_utf8_lossy(&configured.stderr).into_owned());
    }
    let configured = String::from_utf8(configured.stdout).map_err(|error| error.to_string())?;
    let editor = format!("{} \"$@\"", configured.trim());
    let editor = shlex::try_quote(&editor).map_err(|error| error.to_string())?;
    let editor_status = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    // Keep the editor on the terminal while capturing Git's diagnostics separately.
    let wrapper = format!(
        r#"sh -c {editor} dogit-editor "$@" </dev/tty >/dev/tty 2>&1
status=$?
printf '%s' "$status" > "$DOGIT_EDITOR_STATUS"
exit "$status""#
    );
    let wrapper = shlex::try_quote(&wrapper).map_err(|error| error.to_string())?;
    let wrapper = format!("sh -c {wrapper} dogit-editor");
    let output = tempfile::tempfile().map_err(|error| error.to_string())?;
    let status = repo
        .command(spec)
        .arg("--quiet")
        .env("LC_ALL", "C")
        .env("GIT_EDITOR", &wrapper)
        .env("DOGIT_EDITOR_STATUS", editor_status.path())
        .stdin(Stdio::inherit())
        .stdout(output.try_clone().map_err(|error| error.to_string())?)
        .stderr(output.try_clone().map_err(|error| error.to_string())?)
        .status()
        .map_err(|error| error.to_string())?;
    let text = captured_output(output)?.replace(&wrapper, configured.trim());
    let cancelled = std::fs::read_to_string(editor_status.path()).is_ok_and(|status| {
        status == "1"
            || (status == "0"
                && (text.ends_with("Aborting commit due to empty commit message.")
                    || text.ends_with("Aborting commit; you did not edit the message.")))
    });
    if status.success() || cancelled {
        Ok(String::new())
    } else if text.is_empty() {
        Err(format!("{} did not complete ({status})", spec.label))
    } else {
        Err(text)
    }
}

pub(super) fn execute_captured(
    repo: &Repo,
    specs: &[CommandSpec],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    use std::sync::atomic::Ordering;
    let mut details = Vec::new();
    for spec in specs {
        if cancel.load(Ordering::Acquire) || ui_terminal::termination_requested() {
            return Err("Operation interrupted".into());
        }
        let output = tempfile::tempfile().map_err(|error| error.to_string())?;
        let mut command = repo.command(spec);
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .stdout(output.try_clone().map_err(|error| error.to_string())?)
            .stderr(output.try_clone().map_err(|error| error.to_string())?);
        if let Some(bytes) = &spec.stdin {
            let mut input = tempfile::tempfile().map_err(|error| error.to_string())?;
            input.write_all(bytes).map_err(|error| error.to_string())?;
            input.rewind().map_err(|error| error.to_string())?;
            command.stdin(input);
        } else {
            command.stdin(Stdio::null());
        }
        // A separate session also prevents hooks and SSH from opening dogit's /dev/tty.
        let mut child = hostkit::process::ChildGroup::spawn_detached(&mut command)
            .map_err(|error| format!("{}: {error}", spec.label))?;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                break status;
            }
            if cancel.load(Ordering::Acquire) || ui_terminal::termination_requested() {
                return Err("Operation interrupted".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let text = captured_output(output)?;
        if !status.success() {
            return Err(if text.is_empty() {
                format!("{} did not complete", spec.label)
            } else {
                format!("{} failed\n{text}", spec.label)
            });
        }
        details.push(if text.is_empty() {
            format!("{} completed", spec.label)
        } else {
            text
        });
    }
    Ok(details.join("\n"))
}

impl App {
    pub(super) fn handoff(
        &mut self,
        prepared: Prepared,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        let label = self.operation.as_ref().map_or_else(
            || "Git operation".into(),
            |operation| operation.label.clone(),
        );
        drop(terminal.take());
        drop(signals.take());
        let handoff_signals = ui_terminal::SignalGuard::with_options(ui_terminal::SignalOptions {
            reraise_on_drop: false,
            ..Default::default()
        })
        .map_err(|error| error.to_string())?;
        let result = (|| -> Result<String, String> {
            match prepared {
                Prepared::Commit(spec) => execute_commit(&self.repo, &spec),
                Prepared::Editor(editor, path) => {
                    let mut command = Command::new("sh");
                    command
                        .current_dir(&self.repo.root)
                        .arg("-c")
                        .arg(format!("exec {editor} \"$@\""))
                        .arg("dogit-editor")
                        .arg(path);
                    let status = command.status().map_err(|error| error.to_string())?;
                    if status.success() {
                        Ok(String::new())
                    } else {
                        Err("Editor did not complete".into())
                    }
                }
                Prepared::Commands(_)
                | Prepared::Amend { .. }
                | Prepared::Discard(_)
                | Prepared::Abort(_)
                | Prepared::Worktree(_) => {
                    unreachable!("only external editors need a terminal handoff")
                }
            }
        })();
        drop(handoff_signals);
        *signals = Some(ui_terminal::SignalGuard::new().map_err(|error| error.to_string())?);
        *terminal = Some(
            crate::session::Session::new(self.mouse_capture).map_err(|error| error.to_string())?,
        );
        self.operation = None;
        if result.as_ref().is_ok_and(|text| text.is_empty()) {
            self.feedback = None;
        } else {
            self.report_result(&label, result);
        }
        self.refresh();
        self.dirty = true;
        Ok(())
    }
}
