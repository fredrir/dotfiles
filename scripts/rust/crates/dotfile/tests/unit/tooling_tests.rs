use super::*;

#[test]
fn installed_means_the_executable_lives_in_the_dotfiles_bin_directory() {
    let home = Path::new("/home/user");
    assert!(is_installed(
        home,
        Path::new("/home/user/dotfiles/.bin/dotfile")
    ));
    assert!(!is_installed(home, Path::new("/home/user/dotfiles/.bin")));
    assert!(!is_installed(
        home,
        Path::new("/home/user/dotfiles/scripts/rust/target/debug/dotfile")
    ));
    assert!(!is_installed(home, Path::new("/usr/local/bin/dotfile")));
}

#[test]
fn installed_follows_a_symlinked_bin_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let real = temporary.path().join("real-bin");
    fs::create_dir_all(home.join("dotfiles")).unwrap();
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, home.join("dotfiles/.bin")).unwrap();
    assert!(is_installed(&home, &real.join("dotfile")));
}
