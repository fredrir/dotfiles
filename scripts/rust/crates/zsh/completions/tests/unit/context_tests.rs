use super::*;
use testkit::tree;

#[test]
fn ages_read_in_the_largest_whole_unit() {
    assert_eq!(age(1_000, 1_000), "just now");
    assert_eq!(age(1_000, 1_100), "just now");
    assert_eq!(age(3_600, 0), "1h ago");
    assert_eq!(age(90 * 60, 0), "1h ago");
    assert_eq!(age(59 * 60, 0), "59m ago");
    assert_eq!(age(3 * 86_400, 0), "3d ago");
    assert_eq!(age(90 * 86_400, 0), "3mo ago");
}

#[test]
fn paths_expand_from_home_or_the_working_directory() {
    let ctx = Context::testing(Path::new("/h"), Path::new("/w/project"), &[]);
    assert_eq!(ctx.expand("~/x"), PathBuf::from("/h/x"));
    assert_eq!(ctx.expand("~"), PathBuf::from("/h"));
    assert_eq!(ctx.expand("sub"), PathBuf::from("/w/project/sub"));
    assert_eq!(ctx.expand("/abs"), PathBuf::from("/abs"));
}

#[test]
fn the_cache_directory_follows_the_environment() {
    let home = Path::new("/h");
    let explicit = Context::testing(home, home, &[("ZCOMP_CACHE_DIR", "/c")]);
    assert_eq!(explicit.cache_dir(), PathBuf::from("/c"));
    let xdg = Context::testing(
        home,
        home,
        &[("ZCOMP_CACHE_DIR", ""), ("XDG_CACHE_HOME", "/x")],
    );
    assert_eq!(xdg.cache_dir(), PathBuf::from("/x/zcomp"));
    let default = Context::testing(home, home, &[("ZCOMP_CACHE_DIR", "")]);
    assert_eq!(default.cache_dir(), PathBuf::from("/h/.cache/zcomp"));
}

#[test]
fn which_finds_only_executables_on_the_path() {
    let root = tree(&["bin/plain=", "other/"]);
    let bin = root.path().join("bin");
    testkit::executable(&bin.join("tool"), "#!/bin/sh\n");
    let path = format!("{}:{}", root.path().join("other").display(), bin.display());
    let ctx = Context::testing(root.path(), root.path(), &[("PATH", &path)]);
    assert_eq!(ctx.which("tool"), Some(bin.join("tool")));
    assert_eq!(ctx.which("plain"), None);
    assert_eq!(ctx.which("missing"), None);
}

#[test]
fn a_fingerprint_changes_when_the_file_does() {
    let root = tree(&["file=one"]);
    let file = root.path().join("file");
    let before = fingerprint(&file);
    fs::write(&file, "longer contents").unwrap();
    assert_ne!(before, fingerprint(&file));
    assert!(fingerprint(&root.path().join("gone")).ends_with(":missing"));
}
