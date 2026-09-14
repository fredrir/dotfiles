#![forbid(unsafe_code)]

use dotfile_cli::{
    context::Context,
    surface::{completions, metadata},
};
use serde_json::json;
use std::fs;

fn sandbox() -> (tempfile::TempDir, Context) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let home = temp.path().join("home");
    fs::create_dir_all(root.join("config/cli")).unwrap();
    fs::create_dir_all(&home).unwrap();
    let context = Context::new(root.clone(), home.clone(), root.join("config"), home.join(".config"))
        .unwrap();
    (temp, context)
}

#[test]
fn declarative_metadata_generates_scripts_without_python_sources() {
    let (_temp, context) = sandbox();
    let destination = context.home.join("completions");
    let surface = json!({"version":2,"commands":{"example":{"path":["example"],"help":"example","hidden":false,"params":[],"children":[{"path":["example","show"],"help":"show","hidden":false,"params":[{"kind":"argument","name":"target","opts":[],"metavar":"TARGET","help":"target","multiple":false,"required":false,"hidden":false,"completion":{"kind":"call","source":"items"}}],"children":[]}]}}});
    let metadata_path = context.root.join("config/cli/command-surface.json");
    fs::write(&metadata_path, serde_json::to_vec(&surface).unwrap()).unwrap();
    completions::write_all(&context, &destination).unwrap();
    let script = destination.join("tools-completion.zsh");
    let generated = fs::read_to_string(&script).unwrap();
    assert!(generated.contains("compdef _example example"));
    assert!(generated.contains("example __complete items"));
    assert!(generated.contains("compdef _dotfile dotfile"));
    assert!(!context.root.join("scripts/python").exists());
    let before = fs::metadata(&script).unwrap().modified().unwrap();
    completions::write_all(&context, &destination).unwrap();
    assert_eq!(fs::metadata(&script).unwrap().modified().unwrap(), before);
    fs::write(metadata_path, r#"{"version":99,"commands":{}}"#).unwrap();
    assert!(completions::write_all(&context, &destination).is_err());
    assert_eq!(fs::read_to_string(script).unwrap(), generated);
}

#[test]
fn host_completions_follow_the_inventory_override_and_fail_quietly() {
    let (_temp, mut context) = sandbox();
    let hosts = context.root.join("custom-hosts.dotfile");
    fs::write(&hosts, "laptop {\n  ROLE = laptop\n}\n").unwrap();
    context
        .process_env
        .insert("SYSINFO_CONFIG".into(), hosts.clone().into());
    assert_eq!(
        dotfile_cli::surface::values::lines(&context, "hosts", &[]),
        ["laptop:laptop"]
    );
    fs::write(&hosts, "invalid inventory").unwrap();
    assert!(dotfile_cli::surface::values::lines(&context, "hosts", &[]).is_empty());
}

#[cfg(unix)]
#[test]
fn native_metadata_never_executes_a_retired_interpreter_launcher() {
    let (_temp, context) = sandbox();
    let launcher = context.root.join("scripts/rust/target/debug/sysinfo");
    let marker = context.root.join("launched");
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    testkit::executable(
        &launcher,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    assert!(
        metadata::external_many(&context, &["sysinfo".into()])
            .unwrap()
            .is_empty()
    );
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn shared_atomic_writes_refuse_symlinks_and_preserve_modes_and_noop_mtime() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (temp, _) = sandbox();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
    symlink(&target, &link).unwrap();
    assert!(dotfile_cli::fs::write_private(&link, b"private").is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "old");
    assert!(dotfile_cli::fs::write_generated(&target, b"new").unwrap());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o751
    );
    let before = fs::metadata(&target).unwrap().modified().unwrap();
    assert!(!dotfile_cli::fs::write_generated(&target, b"new").unwrap());
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), before);
    assert!(dotfile_cli::fs::write_private(&target, b"new").unwrap());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // A sparse multi-gigabyte existing file must not be read into memory to replace three bytes.
    fs::OpenOptions::new()
        .write(true)
        .open(&target)
        .unwrap()
        .set_len(8 * 1024 * 1024 * 1024)
        .unwrap();
    assert!(dotfile_cli::fs::write_generated(&target, b"new").unwrap());
    assert_eq!(fs::metadata(target).unwrap().len(), 3);
}

#[cfg(unix)]
#[test]
fn repository_lock_spans_state_directories_and_worktrees() {
    use dotfile_cli::lock::MutationLock;
    let (temp, context) = sandbox();
    fs::create_dir(context.root.join(".git")).unwrap();
    let held = MutationLock::acquire(&context).unwrap();
    let mut other = context.clone();
    other.root_config = temp.path().join("other-state");
    assert!(MutationLock::acquire(&other).is_err());
    let worktree = temp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let gitdir = context.root.join(".git/worktrees/other");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    other.root = worktree;
    assert!(MutationLock::acquire(&other).is_err());
    drop(held);
    assert!(MutationLock::acquire(&other).is_ok());
}
