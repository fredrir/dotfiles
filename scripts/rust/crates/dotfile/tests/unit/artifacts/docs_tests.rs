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
