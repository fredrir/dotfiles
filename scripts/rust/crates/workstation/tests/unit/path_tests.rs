use super::*;

#[test]
fn a_named_home_shortens_only_its_real_descendants() {
    let home = Path::new("/Users/f");
    assert_eq!(home_relative_in(Path::new("/Users/f"), home), "~");
    assert_eq!(
        home_relative_in(Path::new("/Users/f/project"), home),
        "~/project"
    );
    assert_eq!(
        home_relative_in(Path::new("/Users/fred/project"), home),
        "/Users/fred/project"
    );
}

#[test]
fn a_named_home_answers_the_paths_hcopy_asks_about() {
    let home = Path::new("/home/f");
    assert_eq!(home_relative_in(Path::new("/home/f/go"), home), "~/go");
    assert_eq!(home_relative_in(Path::new("/home/f"), home), "~");
    assert_eq!(home_relative_in(Path::new("/etc"), home), "/etc");
    assert_eq!(
        home_relative_in(Path::new("/home/fredrir2/go"), home),
        "/home/fredrir2/go"
    );
}

#[test]
fn a_named_home_shortens_the_paths_gitkit_asks_about() {
    let home = Path::new("/home/someone");
    assert_eq!(
        home_relative_in(Path::new("/home/someone/work"), home),
        "~/work"
    );
    assert_eq!(home_relative_in(Path::new("/home/someone"), home), "~");
    assert_eq!(
        home_relative_in(Path::new("/home/someone-else/work"), home),
        "/home/someone-else/work"
    );
}

#[test]
fn the_environment_home_is_the_home_that_is_used() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    assert_eq!(home_relative(&home), "~");
    assert_eq!(home_relative(&home.join("work")), "~/work");
    assert_eq!(home_relative(Path::new("/etc/hosts")), "/etc/hosts");
}

#[test]
fn shortening_prefers_the_current_directory_over_the_home() {
    let here = std::env::current_dir().expect("a current directory");
    assert_eq!(shorten(&here.join("src").join("lib.rs")), "src/lib.rs");
    assert_eq!(shorten(&here), home_relative(&here));
    assert_eq!(shorten(Path::new("/etc/hosts")), "/etc/hosts");
}

#[test]
fn requiring_a_directory_names_what_was_wrong_with_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(require_directory(&root.join("src")), Ok(()));

    let file = root.join("src").join("lib.rs");
    assert_eq!(
        require_directory(&file),
        Err(format!("not a directory: {}", file.display()))
    );

    let missing = root.join("src").join("no-such-module.rs");
    assert_eq!(
        require_directory(&missing),
        Err(format!("no such file or directory: {}", missing.display()))
    );
}

#[test]
fn a_hidden_name_is_one_that_starts_with_a_dot() {
    assert!(hidden(OsStr::new(".git")));
    assert!(hidden(OsStr::new(".")));
    assert!(!hidden(OsStr::new("src")));
    assert!(!hidden(OsStr::new("")));
}
