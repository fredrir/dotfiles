use super::write_private_before_persist;

#[test]
fn failed_private_replace_preserves_existing_secret() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("secret");
    std::fs::write(&path, "original").unwrap();
    let result = write_private_before_persist(&path, b"replacement", |_| {
        Err("injected failure".to_string())
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
}
