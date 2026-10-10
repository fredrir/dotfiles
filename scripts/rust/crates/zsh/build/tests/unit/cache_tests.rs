use std::cell::Cell;
use std::ffi::OsString;

use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn lookup(
    cache: &mut Cache,
    key: Option<Key>,
    runs: &Cell<usize>,
    outcome: Result<Outcome, String>,
) -> Outcome {
    cache.get_or_run(key, || {
        runs.set(runs.get() + 1);
        outcome
    })
}

#[test]
fn an_entry_holds_until_the_program_or_its_search_path_changes() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("tool");
    fs::write(&program, "one").unwrap();
    let path = OsString::from("/usr/bin");
    let argv = argv(&["tool", "--completions", "zsh"]);
    let file = dir.path().join(FILE);
    let runs = Cell::new(0);

    let mut cache = Cache::load(file.clone());
    let key = || Key::new(&argv, &program, Some(&path));
    assert_eq!(
        lookup(&mut cache, key(), &runs, Ok(Ok("out".into()))),
        Ok("out".into())
    );
    assert!(cache.save().unwrap());

    let mut cache = Cache::load(file.clone());
    assert_eq!(
        lookup(&mut cache, key(), &runs, Ok(Ok("other".into()))),
        Ok("out".into())
    );
    assert_eq!(runs.get(), 1);
    assert!(!cache.save().unwrap());

    let other_path = OsString::from("/bin");
    let moved = Key::new(&argv, &program, Some(&other_path));
    assert_eq!(
        lookup(&mut cache, moved, &runs, Ok(Ok("moved".into()))),
        Ok("moved".into())
    );
    fs::write(&program, "two!").unwrap();
    assert_eq!(
        lookup(&mut cache, key(), &runs, Ok(Ok("new".into()))),
        Ok("new".into())
    );
    assert_eq!(runs.get(), 3);
}

#[test]
fn a_symlinked_program_is_keyed_by_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new, link) = (
        dir.path().join("old"),
        dir.path().join("new"),
        dir.path().join("tool"),
    );
    fs::write(&old, "same").unwrap();
    fs::write(&new, "same").unwrap();
    let time = fs::metadata(&old).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(&new)
        .unwrap()
        .set_modified(time)
        .unwrap();
    std::os::unix::fs::symlink(&old, &link).unwrap();
    let argv = argv(&["tool"]);
    let before = Key::new(&argv, &link, None).unwrap();
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&new, &link).unwrap();
    assert_ne!(Key::new(&argv, &link, None).unwrap(), before);
}

#[test]
fn a_script_holds_until_its_checkout_moves_to_another_commit() {
    let dir = tempfile::tempdir().unwrap();
    let checkout = dir.path().join("homebrew");
    let branch = checkout.join(".git/refs/heads/main");
    fs::create_dir_all(branch.parent().unwrap()).unwrap();
    fs::create_dir_all(checkout.join("bin")).unwrap();
    fs::write(checkout.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(&branch, "abc\n").unwrap();
    let (script, binary) = (checkout.join("bin/brew"), checkout.join("bin/tool"));
    fs::write(&script, "#!/bin/bash\n").unwrap();
    fs::write(&binary, "\u{7f}ELF").unwrap();
    let argv = argv(&["brew", "shellenv", "zsh"]);
    let file = dir.path().join(FILE);
    let runs = Cell::new(0);
    let key = || Key::new(&argv, &script, None);

    let mut cache = Cache::load(file.clone());
    lookup(&mut cache, key(), &runs, Ok(Ok("old".into()))).unwrap();
    assert!(cache.save().unwrap());
    let binary_key = Key::new(&argv, &binary, None).unwrap();

    let mut cache = Cache::load(file.clone());
    assert_eq!(
        lookup(&mut cache, key(), &runs, Ok(Ok("other".into()))),
        Ok("old".into())
    );
    fs::write(&branch, "def\n").unwrap();
    assert_eq!(
        lookup(&mut cache, key(), &runs, Ok(Ok("new".into()))),
        Ok("new".into())
    );
    assert_eq!(runs.get(), 2);
    assert_eq!(Key::new(&argv, &binary, None), Some(binary_key));
}

#[test]
fn a_script_without_a_readable_commit_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("bin/tool");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(&script, "#!/bin/sh\n").unwrap();
    let argv = argv(&["tool"]);
    assert_eq!(Key::new(&argv, &script, None), None);

    fs::create_dir_all(dir.path().join(".git")).unwrap();
    assert_eq!(Key::new(&argv, &script, None), None);

    fs::write(dir.path().join("bin/.git"), "gitdir: /elsewhere\n").unwrap();
    fs::write(dir.path().join(".git/HEAD"), "abc\n").unwrap();
    assert_eq!(Key::new(&argv, &script, None), None);
}

#[test]
fn failures_are_kept_unless_worth_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("tool");
    fs::write(&program, "").unwrap();
    let mut cache = Cache::load(dir.path().join(FILE));
    let runs = Cell::new(0);
    let failed = || Key::new(&argv(&["tool", "a"]), &program, None);
    let timed_out = || Key::new(&argv(&["tool", "b"]), &program, None);
    for _ in 0..2 {
        assert_eq!(
            lookup(&mut cache, failed(), &runs, Ok(Err("exit 2".into()))),
            Err("exit 2".into())
        );
        assert_eq!(
            lookup(&mut cache, timed_out(), &runs, Err("timed out".into())),
            Err("timed out".into())
        );
    }
    assert_eq!(runs.get(), 3);
}

#[test]
fn only_the_entries_a_build_used_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("tool");
    fs::write(&program, "").unwrap();
    let file = dir.path().join(FILE);
    let runs = Cell::new(0);
    let key = |arg: &str| Key::new(&argv(&["tool", arg]), &program, None);

    let mut cache = Cache::load(file.clone());
    lookup(&mut cache, key("a"), &runs, Ok(Ok("a".into()))).unwrap();
    lookup(&mut cache, key("b"), &runs, Ok(Ok("b".into()))).unwrap();
    assert!(cache.save().unwrap());

    let mut cache = Cache::load(file.clone());
    lookup(&mut cache, key("a"), &runs, Ok(Ok("x".into()))).unwrap();
    assert!(cache.save().unwrap());

    let mut cache = Cache::load(file.clone());
    assert_eq!(
        lookup(&mut cache, key("b"), &runs, Ok(Ok("b2".into()))),
        Ok("b2".into())
    );
    assert_eq!(runs.get(), 3);

    fs::write(&file, "not json").unwrap();
    let mut cache = Cache::load(file);
    assert_eq!(
        lookup(&mut cache, key("a"), &runs, Ok(Ok("a2".into()))),
        Ok("a2".into())
    );
}

#[test]
fn a_command_runs_with_the_given_search_path() {
    let program = PathBuf::from("/bin/sh");
    let path = OsString::from("/nowhere");
    let printed = run(
        &argv(&["sh", "-c", "printf %s \"$PATH\""]),
        &program,
        Some(&path),
        Duration::from_secs(5),
    );
    assert_eq!(printed, Ok(Ok("/nowhere".into())));
    let failed = run(
        &argv(&["sh", "-c", "exit 3"]),
        &program,
        None,
        Duration::from_secs(5),
    );
    assert!(
        matches!(failed, Ok(Err(ref error)) if error.starts_with("sh -c exit 3: ")),
        "{failed:?}"
    );
}
