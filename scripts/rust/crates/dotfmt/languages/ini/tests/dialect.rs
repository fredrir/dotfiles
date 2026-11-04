#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_ini::dialect::Dialect;

#[test]
fn every_dialect_has_a_name_and_no_duplicate_spelling() {
    assert_eq!(Dialect::ALL.len(), 6);
    let mut names = Vec::new();
    for dialect in Dialect::ALL {
        names.push(dialect.name());
        names.extend(dialect.aliases().iter().copied());
        assert_eq!(
            Dialect::parse(dialect.name(), true).unwrap(),
            dialect,
            "{}",
            dialect.name()
        );
    }
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "{names:?}");
}

#[test]
fn parse_understands_auto_aliases_and_case() {
    assert_eq!(Dialect::parse("auto", true).unwrap(), Dialect::Auto);
    assert_eq!(Dialect::parse("auto", false).unwrap(), Dialect::Auto);
    assert_eq!(Dialect::parse("ini", true).unwrap(), Dialect::Generic);
    assert_eq!(Dialect::parse("gitconfig", true).unwrap(), Dialect::Git);
    assert_eq!(Dialect::parse("GITCONFIG", true).unwrap(), Dialect::Git);
    assert_eq!(Dialect::parse("SSH", true).unwrap(), Dialect::Ssh);
    assert_eq!(
        Dialect::parse("editor-config", true).unwrap(),
        Dialect::EditorConfig
    );
    assert_eq!(Dialect::parse("unit", true).unwrap(), Dialect::Systemd);
    assert_eq!(Dialect::parse("ssh-config", true).unwrap(), Dialect::Ssh);
    assert!(Dialect::parse("nginx", true).is_err());
    assert_eq!("systemd".parse::<Dialect>().unwrap(), Dialect::Systemd);
}

#[test]
fn resolve_prefers_explicit_then_configured_then_the_path() {
    let path = Path::new("/home/u/.gitconfig");
    assert_eq!(
        Dialect::Ssh.resolve(Dialect::Git, path),
        Dialect::Ssh,
        "an explicit dialect wins"
    );
    assert_eq!(
        Dialect::Auto.resolve(Dialect::EditorConfig, path),
        Dialect::EditorConfig,
        "a configured dialect wins over the path"
    );
    assert_eq!(Dialect::Auto.resolve(Dialect::Auto, path), Dialect::Git);
}

#[test]
fn auto_detection_recognizes_git_paths() {
    for path in [
        "/home/u/.gitconfig",
        "/repo/.gitmodules",
        "/repo/.git/config",
        "/home/u/.config/git/config",
    ] {
        assert_eq!(
            Dialect::Auto.resolve(Dialect::Auto, Path::new(path)),
            Dialect::Git,
            "{path}"
        );
    }
}

#[test]
fn auto_detection_recognizes_editorconfig() {
    assert_eq!(
        Dialect::Auto.resolve(Dialect::Auto, Path::new("/repo/.editorconfig")),
        Dialect::EditorConfig
    );
}

#[test]
fn auto_detection_recognizes_systemd_unit_extensions() {
    for extension in [
        "service",
        "timer",
        "path",
        "socket",
        "target",
        "mount",
        "automount",
        "swap",
        "slice",
        "scope",
        "device",
        "network",
        "netdev",
        "link",
    ] {
        let path = format!("/etc/systemd/system/thing.{extension}");
        assert_eq!(
            Dialect::Auto.resolve(Dialect::Auto, Path::new(&path)),
            Dialect::Systemd,
            "{path}"
        );
    }
}

#[test]
fn auto_detection_recognizes_ssh_paths() {
    for path in [
        "/etc/ssh_config",
        "/etc/sshd_config",
        "/home/u/.ssh/config",
        "/home/u/ssh/config",
        "/home/u/.ssh/config.d/work",
        "/etc/ssh/config.d/10-local",
    ] {
        assert_eq!(
            Dialect::Auto.resolve(Dialect::Auto, Path::new(path)),
            Dialect::Ssh,
            "{path}"
        );
    }
}

#[test]
fn auto_detection_falls_back_to_generic() {
    for path in [
        "/tmp/settings.ini",
        "/etc/app.conf",
        "/tmp/no-name",
        "/repo/.git/info/config",
        "/repo/.git/info/exclude",
        "/repo/.git/info/attributes",
        "/tmp/ssh/config/notes",
        "/tmp/ssh/config.d/nested/note",
    ] {
        assert_eq!(
            Dialect::Auto.resolve(Dialect::Auto, Path::new(path)),
            Dialect::Generic,
            "{path}"
        );
    }
}
