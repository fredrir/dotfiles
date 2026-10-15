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
        // Mutations run to completion: a timeout must not turn a slow hook or
        // filesystem into an indeterminate repository operation.
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
                        let command = self.repo.command(&spec);
                        let (status, output) = interactive_command(command)?;
                        if !status.success() {
                            if matches!(spec.label.as_str(), "Commit" | "Amend")
                                && output.contains("Aborting commit due to empty commit message")
                            {
                                result = Ok(String::new());
                            } else {
                                result = Err(format!(
                                    "{} did not complete\n{}",
                                    spec.label,
                                    output.trim()
                                ));
                            }
                            break;
                        }
                        result = Ok(if output.trim().is_empty() {
                            format!("{} completed", spec.label)
                        } else {
                            format!("{} completed\n{}", spec.label, output.trim())
                        });
                    }
                    result
                }
                Prepared::Editor(editor, path) => {
                    let mut command = Command::new("sh");
                    // Git's editor value is a user-authored shell command. The
                    // selected path is passed separately, never interpolated.
                    command
                        .current_dir(&self.repo.root)
                        .arg("-c")
                        .arg(format!("exec {editor} \"$@\""))
                        .arg("dogit-editor")
                        .arg(path);
                    let (status, _) = interactive_command(command)?;
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
        *terminal = Some(crate::session::Session::new().map_err(|error| error.to_string())?);
        self.operation = None;
        if result.as_ref().is_ok_and(|text| text.is_empty()) {
            self.feedback = None;
        } else if !editor || result.is_err() {
            self.record(&label, result);
        }
        self.refresh();
        self.dirty = true;
        Ok(())
    }
}

fn interactive_command(mut command: Command) -> Result<(std::process::ExitStatus, String), String> {
    use std::io::Read;
    #[cfg(target_os = "linux")]
    {
        let transcript = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        let mut recorder = Command::new("script");
        recorder
            .args(["-q", "-e", "-c"])
            .arg(quoted_command(&command))
            .arg(transcript.path());
        if let Some(directory) = command.get_current_dir() {
            recorder.current_dir(directory);
        }
        for (name, value) in command.get_envs() {
            if let Some(value) = value {
                recorder.env(name, value);
            } else {
                recorder.env_remove(name);
            }
        }
        match recorder.status() {
            Ok(status) => {
                let mut bytes = Vec::new();
                std::fs::File::open(transcript.path())
                    .map_err(|error| error.to_string())?
                    .take(256 * 1024)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                let output = String::from_utf8_lossy(&bytes)
                    .lines()
                    .filter(|line| {
                        !line.starts_with("Script started on ")
                            && !line.starts_with("Script done on ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok((status, output));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    command
        .status()
        .map(|status| (status, String::new()))
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn quoted_command(command: &Command) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = Vec::new();
    for argument in std::iter::once(command.get_program()).chain(command.get_args()) {
        if !bytes.is_empty() {
            bytes.push(b' ');
        }
        bytes.push(b'\'');
        for byte in argument.as_encoded_bytes() {
            if *byte == b'\'' {
                bytes.extend_from_slice(b"'\\''");
            } else {
                bytes.push(*byte);
            }
        }
        bytes.push(b'\'');
    }
    std::ffi::OsString::from_vec(bytes)
}
