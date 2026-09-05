use std::os::unix::fs::PermissionsExt;

use super::*;

#[test]
fn documentation_check_cannot_create_python_bytecode() {
    let temporary = tempfile::tempdir().unwrap();
    let module = temporary.path().join("fresh_module.py");
    let backend = temporary.path().join("backend.py");
    fs::write(&module, "VALUE = 1\n").unwrap();
    fs::write(&backend, "#!/usr/bin/env python3\nimport fresh_module\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();

    let output = generate_with_backend(&backend, true).unwrap();

    assert!(output.status.success());
    assert!(!temporary.path().join("__pycache__").exists());
}

#[test]
fn sync_regenerates_keybindings_when_configs_or_pages_change_and_check_never_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/targets.dotfile"), "").unwrap();
    fs::create_dir_all(root.join("shared/tmux")).unwrap();
    let keys = root.join("shared/tmux/keys.conf");
    fs::write(&keys, "bind r refresh-client\n").unwrap();
    let backend = temporary.path().join("backend");
    fs::write(&backend, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
    let context = Context::new(
        root.clone(),
        temporary.path().join("home"),
        temporary.path().join("state"),
    )
    .unwrap();
    let events = crate::event::VecSink::default();

    assert_eq!(
        synchronize_with_backend(&context, true, &events, &backend).unwrap(),
        9
    );
    assert!(!root.join("docs").exists());
    assert!(!context.state.exists());
    assert_eq!(
        synchronize_with_backend(&context, false, &events, &backend).unwrap(),
        9
    );
    assert_eq!(
        synchronize_with_backend(&context, false, &events, &backend).unwrap(),
        0
    );

    fs::write(&keys, "bind z resize-pane -Z\n").unwrap();
    assert_eq!(
        synchronize_with_backend(&context, true, &events, &backend).unwrap(),
        1
    );
    let page = root.join("docs/keybinds/tmux.md");
    assert!(!fs::read_to_string(&page).unwrap().contains("resize-pane"));
    assert_eq!(
        synchronize_with_backend(&context, false, &events, &backend).unwrap(),
        1
    );
    assert!(fs::read_to_string(&page).unwrap().contains("resize-pane"));

    fs::remove_file(&page).unwrap();
    assert_eq!(
        synchronize_with_backend(&context, false, &events, &backend).unwrap(),
        1
    );
    assert!(page.exists());
}
