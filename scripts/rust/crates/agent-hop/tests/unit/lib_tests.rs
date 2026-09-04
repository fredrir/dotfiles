use super::*;

fn remote_session(id: &str, agent: Agent, workspace: &Path) -> remote::RemoteSession {
    remote::RemoteSession {
        agent,
        id: id.to_string(),
        title: String::new(),
        project: String::new(),
        workspace: workspace.to_path_buf(),
        transcript: PathBuf::from(format!("/tmp/{id}.jsonl")),
        companion: None,
        modified_ms: 0,
    }
}

#[test]
fn companion_count_includes_nested_files_without_following_symlinks() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("companion");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("one"), "one").unwrap();
    fs::write(root.join("nested/two"), "two").unwrap();
    assert_eq!(count_companion(Some(&root)).unwrap(), 2);
    assert_eq!(count_companion(None).unwrap(), 0);
}

#[test]
fn child_capture_requires_one_new_session_in_the_destination_workspace() {
    let workspace = Path::new("/home/f/project");
    let before = remote::RemoteCatalog {
        sessions: vec![remote_session("parent", Agent::Codex, workspace)],
        warnings: Vec::new(),
    };
    let after = remote::RemoteCatalog {
        sessions: vec![
            remote_session("parent", Agent::Codex, workspace),
            remote_session("child", Agent::Codex, workspace),
        ],
        warnings: Vec::new(),
    };
    assert_eq!(
        detect_created_child(before.clone(), after, Agent::Codex, workspace, "parent"),
        Some("child".to_string())
    );

    let ambiguous = remote::RemoteCatalog {
        sessions: vec![
            remote_session("parent", Agent::Codex, workspace),
            remote_session("child-one", Agent::Codex, workspace),
            remote_session("child-two", Agent::Codex, workspace),
        ],
        warnings: Vec::new(),
    };
    assert_eq!(
        detect_created_child(before, ambiguous, Agent::Codex, workspace, "parent"),
        None
    );
}
