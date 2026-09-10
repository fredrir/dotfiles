use super::*;

#[test]
fn encrypted_recovery_export_does_not_include_runtime_keys() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let recovery = root.join("recovery.sops.json");
    fs::write(&recovery, "encrypted recovery content").unwrap();
    fs::write(root.join(".sops.yaml"), "creation_rules: []\n").unwrap();
    let config = Config {
        host: "macie".into(),
        secrets_file: Some(recovery),
        password_file: root.join("vanishing/runtime/password"),
        identity_file: root.join("vanishing/runtime/identity"),
        rclone_secrets_file: Some(root.join("rclone.sops.json")),
        rclone_config_file: Some(root.join("vanishing/runtime/rclone")),
        runtime_digest: Some("runtime".into()),
        ..Config::default()
    };
    let target = root.join("export");
    export(&config, &target).unwrap();
    assert!(!target.join("repository.key").exists());
    assert!(!target.join("identity.txt").exists());
    let exported = fs::read_to_string(target.join("config.toml")).unwrap();
    assert!(!exported.contains("vanishing"));
    let loaded = Config::load(&target.join("config.toml")).unwrap();
    assert_eq!(
        loaded.secrets_file.unwrap(),
        target.join("recovery.sops.json")
    );
    assert!(loaded.rclone_config_file.is_none());
}
