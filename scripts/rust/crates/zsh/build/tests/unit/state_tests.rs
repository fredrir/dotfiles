use std::collections::BTreeMap;

use super::*;

fn state() -> State {
    State::new(
        BTreeMap::from([
            (
                "HOMEBREW_PREFIX".to_string(),
                Some("/opt/homebrew".to_string()),
            ),
            ("ABSENT".to_string(), None),
        ]),
        "macie".into(),
        vec![PathBuf::from("/usr/bin")],
    )
}

#[test]
fn unassigned_names_follow_their_origin() {
    let state = state();
    assert_eq!(
        state.var("HOMEBREW_PREFIX"),
        Var::Scalar("/opt/homebrew".into())
    );
    assert_eq!(state.var("ABSENT"), Var::Unset);
    assert_eq!(state.var("plugins"), Var::Unset);
    assert_eq!(state.var("fpath"), Var::Unknown);
    assert_eq!(state.var("VSCODE_INJECTION"), Var::Unknown);
    assert_eq!(state.var("HOST"), Var::Scalar("macie".into()));
}

#[test]
fn path_and_its_array_stay_tied() {
    let mut state = state();
    state.prepend_path(&[PathBuf::from("/opt/bin"), PathBuf::from("/usr/bin")]);
    assert_eq!(
        state.var("path"),
        Var::Array(vec!["/opt/bin".into(), "/usr/bin".into()])
    );
    state.set("PATH", Var::Scalar("/x:/y".into()));
    assert_eq!(
        state.path(),
        Some(&[PathBuf::from("/x"), PathBuf::from("/y")][..])
    );
}

#[test]
fn branches_keep_only_shared_facts() {
    let mut left = state();
    let mut right = state();
    left.set("A", Var::Scalar("1".into()));
    right.set("A", Var::Scalar("1".into()));
    left.set("B", Var::Scalar("1".into()));
    right.prepend_path(&[PathBuf::from("/opt/bin")]);
    left.merge(&right);
    assert_eq!(left.var("A"), Var::Scalar("1".into()));
    assert_eq!(left.var("B"), Var::Unknown);
    assert_eq!(left.path(), None);
}

#[test]
fn locals_are_restored_when_the_frame_ends() {
    let mut state = state();
    state.set("x", Var::Scalar("outer".into()));
    state.push_frame();
    state.declare_local("x");
    state.set("x", Var::Scalar("inner".into()));
    state.set("y", Var::Scalar("global".into()));
    state.pop_frame();
    assert_eq!(state.var("x"), Var::Scalar("outer".into()));
    assert_eq!(state.var("y"), Var::Scalar("global".into()));
}
