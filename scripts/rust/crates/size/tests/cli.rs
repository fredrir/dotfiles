use testkit::{Bin, stderr, stdout};

#[test]
fn completions_are_available() {
    let output = Bin::new(env!("CARGO_BIN_EXE_size"))
        .args(["--completions", "zsh"])
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef size"));
}

#[test]
fn help_describes_this_tool() {
    let output = Bin::new(env!("CARGO_BIN_EXE_size")).arg("--help").output();
    assert!(stdout(&output).starts_with("Sizes and line counts for files and directories"));
}

#[test]
fn a_missing_target_fails() {
    let output = Bin::new(env!("CARGO_BIN_EXE_size"))
        .arg("definitely-not-here")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[cfg(unix)]
mod on_a_terminal {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use testkit::Bin;
    use testkit::pty::{open_pty, read_available, stdio};

    fn listing_on_a_pty(directory: &Path, variables: &[(&str, &str)]) -> Vec<u8> {
        let (master, slave, _) = open_pty(24, 80);
        let (input, output, errors) = stdio(&slave);
        let mut child = Bin::new(env!("CARGO_BIN_EXE_size"))
            .arg("-r")
            .arg(directory)
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR")
            .env("TERM", "xterm-256color")
            .envs(variables.iter().copied())
            .stdio(input, output, errors)
            .spawn();
        drop(slave);

        let mut captured = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            read_available(&master, &mut captured, 100);
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("the listing did not finish before the deadline");
            }
        };
        read_available(&master, &mut captured, 0);
        assert!(status.success(), "{:?}", String::from_utf8_lossy(&captured));
        captured
    }

    fn painted(output: &[u8]) -> bool {
        output.windows(2).any(|window| window == b"\x1b[")
    }

    #[test]
    fn a_terminal_is_painted_unless_the_environment_refuses() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "one\ntwo\n").unwrap();

        let plain = listing_on_a_pty(root.path(), &[]);
        assert!(painted(&plain), "{:?}", String::from_utf8_lossy(&plain));

        let refused = listing_on_a_pty(root.path(), &[("CLICOLOR", "0")]);
        assert!(
            !painted(&refused),
            "{:?}",
            String::from_utf8_lossy(&refused)
        );

        let dumb = listing_on_a_pty(root.path(), &[("TERM", "dumb")]);
        assert!(!painted(&dumb), "{:?}", String::from_utf8_lossy(&dumb));
    }
}
