#![forbid(unsafe_code)]

use dotfile_cli::{
    artifacts::docs,
    context::Context,
    surface::{completions, metadata},
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn sandbox() -> (tempfile::TempDir, Context) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let home = temp.path().join("home");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(&home).unwrap();
    let context = Context::new(root, home, temp.path().join("state")).unwrap();
    (temp, context)
}

#[test]
fn reference_preserves_prose_and_mtime_and_check_does_not_write() {
    let (_temp, context) = sandbox();
    let (changed, _) = docs::generate(&context, false).unwrap();
    assert!(changed.contains(&PathBuf::from("docs/cli/dotfile.md")));
    let path = context.root.join("docs/cli/dotfile.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("intro\n{text}\nclosing\n")).unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(docs::generate(&context, false).unwrap().0.is_empty());
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    let stale = fs::read_to_string(&path)
        .unwrap()
        .replace("Manages this repository", "stale");
    assert!(stale.contains("stale"));
    fs::write(&path, &stale).unwrap();
    assert!(
        docs::generate(&context, true)
            .unwrap()
            .0
            .contains(&PathBuf::from("docs/cli/dotfile.md"))
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), stale);
    docs::generate(&context, false).unwrap();
    let text = fs::read_to_string(path).unwrap();
    assert!(text.starts_with("intro\n"));
    assert!(text.ends_with("closing\n"));
}

#[test]
fn unavailable_external_metadata_keeps_its_document() {
    let (_temp, mut context) = sandbox();
    // Explicitly isolate resolution from installed tools and the caller's prepared build.
    let manifest = context.root.join("build.jsonl");
    fs::write(&manifest, "").unwrap();
    context
        .process_env
        .insert("DOTFILE_DEV_BUILD_MANIFEST".into(), manifest.into());
    let path = context.root.join("docs/cli/count.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "retained\n").unwrap();
    docs::generate(&context, false).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "retained\n");
}

#[test]
fn exported_python_metadata_is_required_and_content_fingerprinted() {
    let (_temp, context) = sandbox();
    let source = context.root.join("scripts/python/src/tools/example.py");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "# source\n").unwrap();
    let destination = context.home.join("completions");
    fs::create_dir(&destination).unwrap();
    let script = destination.join("tools-completion.zsh");
    fs::write(&script, "retained\n").unwrap();
    assert!(
        docs::generate(&context, false)
            .unwrap_err()
            .contains("metadata missing")
    );
    assert!(
        completions::write_all(&context, &destination)
            .unwrap_err()
            .contains("metadata missing")
    );
    assert_eq!(fs::read_to_string(&script).unwrap(), "retained\n");
    let surface = json!({"version":1,"source_fingerprint":metadata::python_fingerprint(&context).unwrap(),"commands":{"example":{"path":["example"],"help":"example","hidden":false,"params":[],"children":[]}},"completions":{"example":"#compdef example\ncompdef _example example\n"}});
    fs::write(
        context.root.join("config/command-surface.json"),
        serde_json::to_vec(&surface).unwrap(),
    )
    .unwrap();
    completions::write_all(&context, &destination).unwrap();
    let generated = fs::read_to_string(&script).unwrap();
    assert!(generated.contains("compdef _example example"));
    assert!(generated.contains("compdef _dotfile dotfile"));
    fs::write(source, "# changed source, same size optional\n").unwrap();
    assert!(
        docs::generate(&context, false)
            .unwrap_err()
            .contains("metadata is stale")
    );
    assert!(completions::write_all(&context, &destination).is_err());
    assert_eq!(fs::read_to_string(script).unwrap(), generated);
}

#[test]
fn prepared_artifact_paths_are_authoritative_and_aliases_resolve() {
    let (_temp, mut context) = sandbox();
    let installed = context.home.join(".local/bin/git-discard");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    fs::write(&installed, "installed").unwrap();
    let prepared = context.root.join("prepared binary");
    fs::write(&prepared, "prepared").unwrap();
    let manifest = context.root.join("build.jsonl");
    fs::write(&manifest, format!("{}\n", json!({"reason":"compiler-artifact","target":{"name":"git-discard"},"executable":prepared}))).unwrap();
    context
        .process_env
        .insert("DOTFILE_DEV_BUILD_MANIFEST".into(), manifest.into());
    assert_eq!(metadata::binary(&context, "gdd").unwrap(), Some(prepared));
    assert_eq!(metadata::binary(&context, "count").unwrap(), None);
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
    other.state = temp.path().join("other-state");
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
