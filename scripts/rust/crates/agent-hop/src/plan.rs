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
mod tests {
    use super::*;

    #[test]
    fn codex_keeps_the_transcript_path_below_its_state_directory() {
        let found = destination(
            Agent::Codex,
            "id",
            Path::new("/Users/f/.codex/sessions/2026/09/02/rollout-id.jsonl"),
            Path::new("/Users/f/projects/app"),
            Path::new("/Users/f"),
            Path::new("/home/f"),
            false,
        )
        .unwrap();
        assert_eq!(found.workspace, Path::new("/home/f/projects/app"));
        assert_eq!(
            found.transcript,
            Path::new("/home/f/.codex/sessions/2026/09/02/rollout-id.jsonl")
        );
        assert!(found.companion.is_none());
    }

    #[test]
    fn claude_uses_the_destination_workspace_for_its_project_key() {
        let found = destination(
            Agent::Claude,
            "session-id",
            Path::new("/Users/f/.claude/projects/-Users-f-app/session-id.jsonl"),
            Path::new("/Users/f/my app/.work"),
            Path::new("/Users/f"),
            Path::new("/home/f"),
            true,
        )
        .unwrap();
        assert_eq!(found.workspace, Path::new("/home/f/my app/.work"));
        assert_eq!(
            found.transcript,
            Path::new("/home/f/.claude/projects/-home-f-my-app--work/session-id.jsonl")
        );
        assert_eq!(
            found.companion.as_deref(),
            Some(Path::new(
                "/home/f/.claude/projects/-home-f-my-app--work/session-id"
            ))
        );
    }

    #[test]
    fn a_home_workspace_maps_to_the_other_home() {
        let found = destination(
            Agent::Claude,
            "id",
            Path::new("/Users/f/.claude/projects/-Users-f/id.jsonl"),
            Path::new("/Users/f"),
            Path::new("/Users/f"),
            Path::new("/home/f"),
            false,
        )
        .unwrap();
        assert_eq!(found.workspace, Path::new("/home/f"));
    }

    #[test]
    fn a_codex_file_outside_its_state_root_is_rejected() {
        assert!(
            destination(
                Agent::Codex,
                "id",
                Path::new("/Users/f/session.jsonl"),
                Path::new("/Users/f/project"),
                Path::new("/Users/f"),
                Path::new("/home/f"),
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn display_shortens_only_real_home_descendants() {
        assert_eq!(display(Path::new("/Users/f"), Path::new("/Users/f")), "~");
        assert_eq!(
            display(Path::new("/Users/f/project"), Path::new("/Users/f")),
            "~/project"
        );
        assert_eq!(
            display(Path::new("/Users/fred/project"), Path::new("/Users/f")),
            "/Users/fred/project"
        );
    }
}
