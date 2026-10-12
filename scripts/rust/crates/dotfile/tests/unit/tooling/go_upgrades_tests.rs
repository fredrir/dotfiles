#![cfg(unix)]

use super::*;

use std::fs::File;

use crate::event::VecSink;
use testkit::{TempDir, executable, tree_pairs};

const GO: &str = r#"#!/bin/sh
case "$1 $2" in
  "list -m")
    [ -n "$GO_OFFLINE" ] && exit 1
    if [ -n "$GO_UPDATE" ]; then
      printf '{"Path": "%s", "Version": "v1.0.0", "Update": {"Version": "%s"}}' "$5" "$GO_UPDATE"
    else
      printf '{"Path": "%s", "Version": "v1.0.0"}' "$5"
    fi ;;
  "mod download") printf '{"Dir": "%s"}' "$GO_MODULE_DIR" ;;
  "mod tidy") printf 'tidied\n' >> go.sum ;;
  get*) printf '// %s\n' "$2" >> go.mod ;;
esac
"#;

const GO_MOD: &str = "module dotfiles/tools\n\nrequire example.com/lib v1.0.0\n";
const GO_SUM: &str = "example.com/lib v1.0.0 h1:abc\n";
const PATCH: &str = "--- a/hello.go\n+++ b/hello.go\n@@ -1,3 +1,3 @@\n package lib\n \n-const Greeting = \"hi\"\n+const Greeting = \"hello\"\n";

struct Sandbox {
    temporary: TempDir,
    context: Context,
}

impl Sandbox {
    fn new(module_source: &str) -> Self {
        let temporary = tree_pairs(&[
            ("repo/config/targets.dotfile", ""),
            ("repo/scripts/go/go.mod", GO_MOD),
            ("repo/scripts/go/go.sum", GO_SUM),
            ("repo/scripts/go/patches/example.com/lib.patch", PATCH),
            ("modcache/lib/go.mod", "module example.com/lib\n"),
            ("modcache/lib/hello.go", module_source),
            ("stubs/", ""),
        ]);
        let stubs = temporary.path().join("stubs");
        executable(&stubs.join("go"), GO);
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("git"))
            .find(|candidate| candidate.is_file())
            .expect("git is on PATH");
        std::os::unix::fs::symlink(git, stubs.join("git")).unwrap();
        let root = temporary.path().join("repo");
        let mut context = Context::new(
            root.clone(),
            temporary.path().join("home"),
            root.join("config"),
            temporary.path().join("home/.config"),
        )
        .unwrap();
        context.process_env.insert("PATH".into(), stubs.into());
        context.process_env.insert(
            "GO_MODULE_DIR".into(),
            temporary.path().join("modcache/lib").into(),
        );
        Self { temporary, context }
    }

    fn offer(&mut self, version: &str) {
        self.context
            .process_env
            .insert("GO_UPDATE".into(), version.into());
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.temporary.path().join("repo").join(relative)).unwrap()
    }

    fn upgrade(&self) -> Vec<Event> {
        let events = VecSink::default();
        upgrade_patched(&self.context, &events).unwrap();
        events.events()
    }
}

const SOURCE: &str = "package lib\n\nconst Greeting = \"hi\"\n";

#[test]
fn the_check_is_due_once_a_day() {
    let sandbox = Sandbox::new(SOURCE);
    assert!(due(&sandbox.context));

    sandbox.upgrade();
    assert!(!due(&sandbox.context));

    File::options()
        .write(true)
        .open(stamp(&sandbox.context))
        .unwrap()
        .set_modified(SystemTime::now() - INTERVAL - Duration::from_secs(60))
        .unwrap();
    assert!(due(&sandbox.context));
}

#[test]
fn nothing_is_due_without_patches() {
    let sandbox = Sandbox::new(SOURCE);
    fs::remove_dir_all(sandbox.temporary.path().join("repo/scripts/go/patches")).unwrap();

    assert!(!due(&sandbox.context));
}

#[test]
fn a_newer_release_is_kept_when_its_patch_applies() {
    let mut sandbox = Sandbox::new(SOURCE);
    sandbox.offer("v1.1.0");

    let events = sandbox.upgrade();

    assert!(
        sandbox
            .read("scripts/go/go.mod")
            .contains("example.com/lib@v1.1.0")
    );
    assert!(sandbox.read("scripts/go/go.sum").contains("tidied"));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Item { detail, changed: true, .. } if detail == "example.com/lib v1.1.0"
    )));
}

#[test]
fn a_newer_release_is_kept_back_when_its_patch_no_longer_applies() {
    let mut sandbox = Sandbox::new("package lib\n\nconst Greeting = \"hey\"\n");
    sandbox.offer("v1.1.0");

    let events = sandbox.upgrade();

    assert_eq!(sandbox.read("scripts/go/go.mod"), GO_MOD);
    assert_eq!(sandbox.read("scripts/go/go.sum"), GO_SUM);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Warning { message, .. } if message.starts_with("example.com/lib v1.1.0 kept back")
    )));
    assert!(!due(&sandbox.context));
}

#[test]
fn an_unreachable_proxy_changes_nothing_until_the_next_day() {
    let mut sandbox = Sandbox::new(SOURCE);
    sandbox.offer("v1.1.0");
    sandbox
        .context
        .process_env
        .insert("GO_OFFLINE".into(), "1".into());

    let events = sandbox.upgrade();

    assert_eq!(sandbox.read("scripts/go/go.mod"), GO_MOD);
    assert!(events.is_empty());
    assert!(!due(&sandbox.context));
}

#[test]
fn the_current_release_changes_nothing() {
    let sandbox = Sandbox::new(SOURCE);

    let events = sandbox.upgrade();

    assert_eq!(sandbox.read("scripts/go/go.mod"), GO_MOD);
    assert!(events.is_empty());
}
