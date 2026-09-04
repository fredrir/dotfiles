use std::path::{Path, PathBuf};

use crate::cli::Agent;
use crate::session::{claude_project_key, workspace_relative};

pub struct Destination {
    pub workspace: PathBuf,
    pub transcript: PathBuf,
    pub companion: Option<PathBuf>,
}

pub fn destination(
    agent: Agent,
    session_id: &str,
    source_transcript: &Path,
    source_workspace: &Path,
    source_home: &Path,
    remote_home: &Path,
    has_companion: bool,
) -> Result<Destination, String> {
    if !remote_home.is_absolute() {
        return Err("the other machine returned a non-absolute home directory".into());
    }
    let workspace = remote_home.join(workspace_relative(source_home, source_workspace)?);
    let (transcript, companion) = match agent {
        Agent::Codex => {
            let relative = source_transcript
                .strip_prefix(source_home.join(".codex"))
                .map_err(|_| "the Codex transcript is outside ~/.codex".to_string())?;
            (remote_home.join(".codex").join(relative), None)
        }
        Agent::Claude => {
            let directory = remote_home
                .join(".claude/projects")
                .join(claude_project_key(&workspace)?);
            let transcript = directory.join(format!("{session_id}.jsonl"));
            let companion = has_companion.then(|| directory.join(session_id));
            (transcript, companion)
        }
    };
    Ok(Destination {
        workspace,
        transcript,
        companion,
    })
}

pub fn display(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".into(),
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
#[path = "../tests/unit/plan_tests.rs"]
mod tests;
