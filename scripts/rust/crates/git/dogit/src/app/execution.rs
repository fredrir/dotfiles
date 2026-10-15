use super::*;

pub(super) fn execute_captured(repo: &Repo, specs: &[CommandSpec]) -> Result<String, String> {
    use std::io::{Read, Seek};
    let mut details = Vec::new();
    for spec in specs {
        let mut output = tempfile::tempfile().map_err(|error| error.to_string())?;
        let mut command = repo.command(spec);
        command
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
        let status = command.status().map_err(|error| error.to_string())?;
        output.rewind().map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        output
            .take(256 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let text = String::from_utf8_lossy(&bytes).trim().to_owned();
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
        let editor = matches!(prepared, Prepared::Editor(..));
        drop(terminal.take());
        drop(signals.take());
        let handoff_signals = ui_terminal::SignalGuard::with_options(ui_terminal::SignalOptions {
            reraise_on_drop: false,
            ..Default::default()
        })
        .map_err(|error| error.to_string())?;
        let result = (|| -> Result<String, String> {
            match prepared {
                Prepared::Commands(commands) => {
                    let mut result = Ok(String::new());
                    for spec in commands {
                        let status = self
                            .repo
                            .command(&spec)
                            .status()
                            .map_err(|error| error.to_string())?;
                        if !status.success() {
                            result = Err(format!("{} did not complete", spec.label));
                            break;
                        }
                        result = Ok(format!("{} completed", spec.label));
                    }
                    result
                }
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
                Prepared::Discard(_) | Prepared::Abort(_) | Prepared::Worktree(_) => {
                    unreachable!("discard previews are handled before terminal handoff")
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
        } else if !editor || result.is_err() {
            self.report_result(&label, result);
        }
        self.refresh();
        self.dirty = true;
        Ok(())
    }
}
