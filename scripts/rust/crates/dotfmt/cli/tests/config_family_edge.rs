#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn git_config_from_inside_dot_git_uses_git_dialect() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "ini {}\n"),
        (".git/config", "[core]\n\teditor   =   vim\n"),
    ]);
    let output = dotfmt(root.path())
        .current_dir(root.path().join(".git"))
        .arg(".")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join(".git/config")).unwrap(),
        "[core]\n  editor = vim\n"
    );
}

#[test]
fn xdg_git_config_from_inside_git_directory_uses_git_dialect() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "ini {}\n"),
        (".config/git/config", "[user]\n\temail   =   a@b\n"),
    ]);
    let output = dotfmt(root.path())
        .current_dir(root.path().join(".config/git"))
        .arg(".")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join(".config/git/config")).unwrap(),
        "[user]\n  email = a@b\n"
    );
}

#[test]
fn git_info_attributes_from_inside_info_uses_attributes_dialect() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "ignore {}\n"),
        (".git/info/attributes", "*.bin   -text\n"),
    ]);
    let output = dotfmt(root.path())
        .current_dir(root.path().join(".git/info"))
        .arg(".")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join(".git/info/attributes")).unwrap(),
        "*.bin -text"
    );
}

#[test]
fn ssh_config_from_inside_dot_ssh_uses_ssh_dialect() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "ini {}\n"),
        (".ssh/config", "Host example\n  HostName    example.com\n"),
        (".ssh/config.d/50-servers", "Host inner\n  User    root\n"),
    ]);
    let output = dotfmt(root.path())
        .current_dir(root.path().join(".ssh"))
        .arg(".")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join(".ssh/config")).unwrap(),
        "Host example\n    HostName example.com\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".ssh/config.d/50-servers")).unwrap(),
        "Host inner\n    User root\n"
    );
}

#[test]
fn hypr_config_from_inside_hypr_directory_uses_hypr_mode() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "conf {}\n"),
        (
            ".config/hypr/hyprland.conf",
            "general{\ngaps_in=5\n  border_size   =   2\n\n}\n",
        ),
    ]);
    let output = dotfmt(root.path())
        .current_dir(root.path().join(".config/hypr"))
        .arg(".")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join(".config/hypr/hyprland.conf")).unwrap(),
        "general{\n    gaps_in = 5\n    border_size = 2\n}"
    );
}

#[test]
fn stdin_config_from_inside_dot_git_uses_git_dialect() {
    let root = tree_pairs(&[("dotfmt.dotfile", "ini {}\n"), (".git/config", "")]);
    let output = dotfmt(root.path())
        .current_dir(root.path().join(".git"))
        .args(["--stdin", "config"])
        .stdin("[core]\n\teditor   =   vim\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "[core]\n  editor = vim\n");
}
