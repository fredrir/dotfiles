use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use hostkit::Host;

use crate::cli::Agent;

const CONNECT_TIMEOUT: &str = "ConnectTimeout=8";
const LOG_LEVEL: &str = "LogLevel=ERROR";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Remote {
    peer: Host,
}

impl Remote {
    pub fn new(peer: Host) -> Remote {
        Remote { peer }
    }

    pub fn peer(self) -> Host {
        self.peer
    }

    pub fn home(self) -> Result<PathBuf, String> {
        let output = self.output(home_script())?;
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| format!("{} returned a non-UTF-8 home directory", self.peer.name()))?;
        let text = text.strip_suffix('\n').unwrap_or(text);
        let text = text.strip_suffix('\r').unwrap_or(text);
        if text.is_empty() || text.contains('\r') || text.contains('\n') {
            return Err(format!(
                "{} returned an invalid home directory",
                self.peer.name()
            ));
        }
        let home = PathBuf::from(text);
        if !home.is_absolute() {
            return Err(format!(
                "{} returned an invalid home directory",
                self.peer.name()
            ));
        }
        Ok(home)
    }

    pub fn preflight(self, workspace: &Path, agent: Agent) -> Result<(), String> {
        self.output(&preflight_script(workspace, agent)?)?;
        Ok(())
    }

    pub fn exists(self, path: &Path) -> Result<bool, String> {
        let output = self.output(&exists_script(path)?)?;
        match output.stdout.as_slice() {
            b"yes\n" => Ok(true),
            b"no\n" => Ok(false),
            _ => Err(format!(
                "{} returned an invalid file status",
                self.peer.name()
            )),
        }
    }

    pub fn file_matches(self, local: &Path, remote: &Path) -> Result<bool, String> {
        let file = File::open(local)
            .map_err(|error| format!("could not open {}: {error}", local.display()))?;
        let output = Command::new("ssh")
            .args(ssh_arguments(self.peer, &compare_script(remote)?, false))
            .stdin(Stdio::from(file))
            .output()
            .map_err(command_error)?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(output_error(self.peer, &output)),
        }
    }

    pub fn mkdir(self, path: &Path) -> Result<(), String> {
        self.output(&mkdir_script(path)?)?;
        Ok(())
    }

    pub fn launch(self, workspace: &Path, agent: Agent, session_id: &str) -> Result<(), String> {
        let script = launch_script(workspace, agent, session_id)?;
        let status = Command::new("ssh")
            .args(ssh_arguments(self.peer, &script, true))
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(command_error)?;
        if status.success() {
            Ok(())
        } else {
            Err(match status.code() {
                Some(code) => format!("{} session exited with status {code}", self.peer.name()),
                None => format!("{} session was interrupted", self.peer.name()),
            })
        }
    }

    fn output(self, script: &str) -> Result<Output, String> {
        let output = Command::new("ssh")
            .args(ssh_arguments(self.peer, script, false))
            .output()
            .map_err(command_error)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(output_error(self.peer, &output))
        }
    }
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn home_script() -> &'static str {
    "printf '%s\\n' \"$HOME\""
}

pub fn preflight_script(workspace: &Path, agent: Agent) -> Result<String, String> {
    let workspace = quote_path(workspace)?;
    let agent = shell_quote(agent.name());
    Ok(format!(
        "test -d {workspace} || {{ printf '%s\\n' 'workspace does not exist' >&2; exit 1; }}; \
         command -v {agent} >/dev/null 2>&1 || {{ printf '%s\\n' 'agent command is not available' >&2; exit 1; }}; \
         command -v 'zsh' >/dev/null 2>&1 || {{ printf '%s\\n' 'zsh is not available' >&2; exit 1; }}"
    ))
}

pub fn exists_script(path: &Path) -> Result<String, String> {
    let path = quote_path(path)?;
    Ok(format!(
        "if [ -e {path} ] || [ -L {path} ]; then printf 'yes\\n'; else printf 'no\\n'; fi"
    ))
}

pub fn compare_script(path: &Path) -> Result<String, String> {
    Ok(format!("cmp -s - {}", quote_path(path)?))
}

pub fn mkdir_script(path: &Path) -> Result<String, String> {
    Ok(format!("mkdir -p -- {}", quote_path(path)?))
}

pub fn fork_command(workspace: &Path, agent: Agent, session_id: &str) -> Result<String, String> {
    let session_id = shell_quote(session_id);
    Ok(match agent {
        Agent::Codex => format!("codex fork {session_id} -C {}", quote_path(workspace)?),
        Agent::Claude => format!("claude --resume {session_id} --fork-session"),
    })
}

pub fn launch_script(workspace: &Path, agent: Agent, session_id: &str) -> Result<String, String> {
    let inner = fork_command(workspace, agent, session_id)?;
    Ok(format!(
        "cd -- {} && exec zsh -lic {}",
        quote_path(workspace)?,
        shell_quote(&inner)
    ))
}

pub fn ssh_arguments(peer: Host, script: &str, interactive: bool) -> Vec<OsString> {
    vec![
        OsString::from(if interactive { "-tt" } else { "-T" }),
        OsString::from("-o"),
        OsString::from(CONNECT_TIMEOUT),
        OsString::from("-o"),
        OsString::from(LOG_LEVEL),
        OsString::from(peer.name()),
        OsString::from(script),
    ]
}

fn quote_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(shell_quote)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn command_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "ssh is required".to_string()
    } else {
        format!("ssh: {error}")
    }
}

fn output_error(peer: Host, output: &Output) -> String {
    let reason = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string);
    match (reason, output.status.code()) {
        (Some(reason), _) => format!("{}: {reason}", peer.name()),
        (None, Some(code)) => format!("{}: ssh exited with status {code}", peer.name()),
        (None, None) => format!("{}: ssh was interrupted", peer.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_values_are_single_quoted_without_losing_apostrophes() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's $HOME"), "'it'\\''s $HOME'");
    }

    #[test]
    fn every_remote_path_is_quoted_as_one_shell_word() {
        let path = Path::new("/home/fred rir/a'b; touch nope");
        assert_eq!(
            exists_script(path).unwrap(),
            "if [ -e '/home/fred rir/a'\\''b; touch nope' ] || [ -L '/home/fred rir/a'\\''b; touch nope' ]; then printf 'yes\\n'; else printf 'no\\n'; fi"
        );
        assert_eq!(
            compare_script(path).unwrap(),
            "cmp -s - '/home/fred rir/a'\\''b; touch nope'"
        );
        assert_eq!(
            mkdir_script(path).unwrap(),
            "mkdir -p -- '/home/fred rir/a'\\''b; touch nope'"
        );
    }

    #[test]
    fn preflight_checks_the_workspace_agent_and_login_shell() {
        let script = preflight_script(Path::new("/home/fred rir/project"), Agent::Codex).unwrap();
        assert!(script.starts_with("test -d '/home/fred rir/project'"));
        assert!(script.contains("command -v 'codex'"));
        assert!(script.contains("command -v 'zsh'"));
        assert!(script.contains("workspace does not exist"));
    }

    #[test]
    fn codex_forks_in_the_mapped_workspace() {
        assert_eq!(
            fork_command(Path::new("/home/fred rir/project"), Agent::Codex, "id'$(x)").unwrap(),
            "codex fork 'id'\\''$(x)' -C '/home/fred rir/project'"
        );
    }

    #[test]
    fn claude_forks_without_interpreting_the_session_id() {
        assert_eq!(
            fork_command(
                Path::new("/home/fred rir/project"),
                Agent::Claude,
                "id; reboot"
            )
            .unwrap(),
            "claude --resume 'id; reboot' --fork-session"
        );
    }

    #[test]
    fn launch_enters_the_workspace_through_an_interactive_login_zsh() {
        let script =
            launch_script(Path::new("/home/fred rir/project"), Agent::Codex, "0199-id").unwrap();
        assert_eq!(
            script,
            r#"cd -- '/home/fred rir/project' && exec zsh -lic 'codex fork '\''0199-id'\'' -C '\''/home/fred rir/project'\'''"#
        );
    }

    #[test]
    fn noninteractive_ssh_has_no_tty_and_keeps_the_script_one_argument() {
        assert_eq!(
            ssh_arguments(Host::Archie, "test -d '/a b'", false),
            [
                "-T",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "LogLevel=ERROR",
                "archie",
                "test -d '/a b'",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn interactive_ssh_allocates_a_tty_for_the_agent() {
        let arguments = ssh_arguments(Host::Macie, "exec zsh -lic 'codex'", true);
        assert_eq!(arguments[0], "-tt");
        assert_eq!(arguments[5], "macie");
        assert_eq!(arguments[6], "exec zsh -lic 'codex'");
    }
}
