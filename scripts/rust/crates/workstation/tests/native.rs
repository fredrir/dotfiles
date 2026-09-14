#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;
use workstation::native::Resolver;

fn native(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"\x7fELFfixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn executable_sibling_wins_and_fallbacks_ignore_modification_times() {
    let temp = tempfile::tempdir().unwrap();
    let mut resolver = Resolver {
        root: temp.path().join("repo"),
        home: temp.path().join("home"),
        current_exe: None,
        manifest: None,
    };
    let debug = resolver.root.join("scripts/rust/target/debug/tool");
    let release = resolver.root.join("scripts/rust/target/release/tool");
    let installed = resolver.home.join("dotfiles/.bin/tool");
    for path in [&release, &installed, &debug] {
        native(path);
    }
    assert_eq!(resolver.resolve("tool").unwrap(), Some(release.clone()));
    resolver.current_exe = Some(resolver.home.join("dotfiles/.bin/dotfile"));
    assert_eq!(resolver.resolve("tool").unwrap(), Some(installed.clone()));
    resolver.current_exe = Some(
        resolver
            .root
            .join("scripts/rust/target/debug/deps/test-123"),
    );
    assert_eq!(resolver.resolve("tool").unwrap(), Some(debug.clone()));
    fs::remove_file(debug).unwrap();
    assert_eq!(resolver.resolve("tool").unwrap(), Some(release.clone()));
    fs::remove_file(release).unwrap();
    assert_eq!(resolver.resolve("tool").unwrap(), Some(installed));
}

#[test]
fn prepared_manifest_is_authoritative_and_test_harnesses_are_not_tools() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("build.jsonl");
    let resolver = Resolver {
        root: temp.path().join("repo"),
        home: temp.path().join("home"),
        current_exe: None,
        manifest: Some(manifest.clone()),
    };
    native(&resolver.home.join(".local/bin/count"));
    let prepared = temp.path().join("prepared command");
    native(&prepared);
    let harness = temp.path().join("test harness");
    native(&harness);
    fs::write(&manifest, format!("{}\n{}\n", serde_json::json!({"reason":"compiler-artifact","target":{"name":"git-discard"},"profile":{"test":false},"executable":prepared}), serde_json::json!({"reason":"compiler-artifact","target":{"name":"git-discard"},"profile":{"test":true},"executable":harness}))).unwrap();
    assert_eq!(resolver.resolve("gdd").unwrap(), Some(prepared));
    assert_eq!(resolver.resolve("count").unwrap(), None);
    assert!(resolver.resolve("../count").is_err());
}

#[test]
fn interpreter_scripts_and_nonexecutables_are_excluded() {
    let temp = tempfile::tempdir().unwrap();
    let resolver = Resolver {
        root: temp.path().join("repo"),
        home: temp.path().join("home"),
        current_exe: None,
        manifest: None,
    };
    let launcher = resolver.home.join(".local/bin/sysinfo");
    native(&launcher);
    fs::write(
        &launcher,
        "#!/usr/bin/env python3\nraise RuntimeError('never launch')\n",
    )
    .unwrap();
    assert!(resolver.resolve("sysinfo").unwrap().is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        native(&launcher);
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolver.resolve("sysinfo").unwrap().is_none());
    }
}
