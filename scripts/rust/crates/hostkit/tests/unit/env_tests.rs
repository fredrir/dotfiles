use super::*;

#[test]
fn a_missing_runtime_dir_falls_back_to_the_users_run_directory() {
    assert_eq!(runtime_dir(None, 1000), "/run/user/1000");
    assert_eq!(runtime_dir(Some(OsString::new()), 1000), "/run/user/1000");
}

#[test]
fn an_existing_runtime_dir_is_kept() {
    assert_eq!(
        runtime_dir(Some(OsString::from("/run/user/42")), 1000),
        "/run/user/42"
    );
}
