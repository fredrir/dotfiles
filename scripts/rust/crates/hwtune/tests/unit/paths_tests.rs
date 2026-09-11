use super::*;

fn paths() -> Paths {
    Paths {
        root: PathBuf::from("/repo"),
        host: "archie".into(),
    }
}

#[test]
fn files_are_named_after_the_host() {
    let paths = paths();
    assert_eq!(
        paths.spec_file(),
        PathBuf::from("/repo/config/bios/archie.dotfile")
    );
    assert_eq!(
        paths.stability_file(),
        PathBuf::from("/repo/config/bios/archie-stability.dotfile")
    );
    assert_eq!(
        paths.exports_dir(),
        PathBuf::from("/repo/config/bios/exports")
    );
}

#[test]
fn explicit_host_wins() {
    assert_eq!(host_name(Some("macie")).unwrap(), "macie");
}
