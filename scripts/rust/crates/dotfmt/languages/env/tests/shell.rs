#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use dotfmt_env::config::Config;
use dotfmt_env::format;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

const SHELL_COMPATIBLE: &[&str] = &[
    "LONG_NAME= \"=.bc#:X/XX\"\n",
    "LONG_NAME= \"=.bc#:X/XX\"\nA=1\n",
    "LONG_NAME =\"=.bc#:X/XX\"\n",
    "LONG_NAME = \"=.bc#:X/XX\"\n",
    "KEY = value\nKEY2= value\nKEY3 = value\nKEY4=value\n",
    "A=1\nB = 2\nC= 3\nD = 4\n",
    "A = 1\nexport B = 2\nexport C= 3\nexport D =4\n",
    "A= x\nB =y\nC = z\n",
    "A=\nB= \nC=   \n",
    "A= \nB = \n",
    "A=1\nA=2\nB=1\n",
    "export A=1\nexport A=2\n",
    "A=value # comment\nA2=value  # comment\n",
    "A='value # not comment'\nB=\"value # not comment\"\n",
    "A=https://host/#frag\nB=value#frag\nC=#frag\n",
    "A=foo\\ \nB=bar\\\t\n",
    "A=foo\\ #bar\nB=foo\\  #bar\n",
    "A=\\$literal\nB=$HOME\nC=${HOME}\n",
    "A='a=b=c'\nB=\"a=b=c\"\nC=a=b=c\n",
    "  A=1\n\tB=2\n  export C=3\n",
    "A=1   \nB=2\t\n",
    "A=1 #c   \n# comment  \nB=2\n",
    "#foo\\ \nB=2\n",
    "A=1\n\n\nB=2\n",
    "A=1\nB=2",
    "KEY\nA=1\n",
    "A=`true`\nB=2\n",
    "A=foo\\\n  #joined\nB=2\n",
    "A=foo\\\n#glued\nB=2\n",
    "A=foo\\\n  #joined  \nB=2\n",
    "A=foo\\\n\n  #standalone\nB=2\n",
];

fn existing(paths: &[&'static str]) -> Option<&'static str> {
    paths.iter().copied().find(|path| Path::new(path).is_file())
}

fn sourced_environment(bash: &str, env: &str, path: &Path) -> BTreeMap<String, String> {
    let output = Command::new(env)
        .args([
            "-i",
            "PATH=/usr/bin:/bin",
            "HOME=/tmp",
            bash,
            "--noprofile",
            "--norc",
            "-c",
            "set -a; . \"$1\" 2>/dev/null; \"$2\"",
            "bash",
        ])
        .arg(path)
        .arg(env)
        .output()
        .expect("bash and env should run");
    let mut environment = BTreeMap::new();
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let (key, value) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("unexpected env line: {line:?}"));
        if key != "_" {
            environment.insert(key.to_owned(), value.to_owned());
        }
    }
    environment
}

#[test]
#[cfg(unix)]
fn formatting_keeps_shell_assignments_identical() {
    let Some(bash) = existing(&["/bin/bash", "/usr/bin/bash"]) else {
        return;
    };
    let Some(env) = existing(&["/usr/bin/env", "/bin/env"]) else {
        return;
    };
    let case = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("dotfmt-env-shell-{}-{case}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    for (index, input) in SHELL_COMPATIBLE.iter().enumerate() {
        let formatted = format(input, &Config::default()).unwrap();
        let before = dir.join(format!("{index}-before.env"));
        let after = dir.join(format!("{index}-after.env"));
        fs::write(&before, input).unwrap();
        fs::write(&after, &formatted).unwrap();
        assert_eq!(
            sourced_environment(bash, env, &before),
            sourced_environment(bash, env, &after),
            "input {input:?} formatted to {formatted:?}"
        );
    }

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_comment_backslash_before_trailing_space_does_not_swallow_the_next_line() {
    let input = "#foo\\ \nB=2\n";
    let output = dotfmt_env::format(input, &Config::default()).unwrap();
    assert_eq!(output, input);
}
