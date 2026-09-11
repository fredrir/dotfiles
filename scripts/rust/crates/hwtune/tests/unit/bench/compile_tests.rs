use super::*;

#[test]
fn embedded_workload_is_pinned_and_named() {
    assert_eq!(lock_packages(LOCKFILE), 162);
    assert!(LOCKFILE.contains("name = \"hwtune-compile-pinned\""));
    assert!(MANIFEST.contains("name = \"hwtune-compile-pinned\""));
    assert!(SOURCE.contains("fn main"));
    assert_eq!(METHOD, "compile.pinned/1.0.0");
}

#[test]
fn lockfile_identity_is_a_stable_sha256() {
    let sha = lock_sha(LOCKFILE);
    assert_eq!(sha.len(), 64);
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(sha, lock_sha(LOCKFILE));
    assert_ne!(sha, lock_sha("[[package]]\n"));
    assert_eq!(
        lock_packages("[[package]]\nname = \"a\"\n\n[[package]]\n"),
        2
    );
}

#[test]
fn assets_are_written_once_into_the_compile_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = workspace(temp.path());
    assert_eq!(root, temp.path().join("compile"));
    assert_eq!(target_dir(&root), root.join("target"));
    write_assets(&root).unwrap();
    for name in ["Cargo.toml", "Cargo.lock", "src/main.rs"] {
        assert!(root.join(name).is_file(), "{name}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("Cargo.lock")).unwrap(),
        LOCKFILE
    );
    assert!(!write_if_changed(&root.join("Cargo.lock"), LOCKFILE).unwrap());
    assert!(write_if_changed(&root.join("Cargo.lock"), "changed").unwrap());
    write_assets(&root).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("Cargo.lock")).unwrap(),
        LOCKFILE
    );
}

#[test]
fn target_cleanup_tolerates_absence() {
    let temp = tempfile::tempdir().unwrap();
    let root = workspace(temp.path());
    clean_target(&root).unwrap();
    std::fs::create_dir_all(target_dir(&root).join("debug")).unwrap();
    clean_target(&root).unwrap();
    assert!(!target_dir(&root).exists());
}

#[test]
fn build_commands_are_offline_locked_and_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let root = workspace(temp.path());
    let cargo = Path::new("/usr/bin/cargo");
    for release in [false, true] {
        let command = build_command(cargo, &root, release);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[..4], ["build", "-q", "--locked", "--offline"]);
        assert_eq!(args.contains(&"--release".to_string()), release);
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(envs["CARGO_INCREMENTAL"].as_deref(), Some("0"));
        assert_eq!(
            envs["CARGO_TARGET_DIR"].as_deref(),
            Some(target_dir(&root).to_str().unwrap())
        );
        assert_eq!(
            envs["CARGO_BUILD_BUILD_DIR"].as_deref(),
            Some(target_dir(&root).to_str().unwrap())
        );
        assert_eq!(envs["RUSTC_WRAPPER"].as_deref(), Some(""));
        assert_eq!(envs["RUSTFLAGS"].as_deref(), Some(""));
    }
}
