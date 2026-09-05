use std::process::Output;

use hostkit::Host;
use testkit::{Bin, stderr, stdout};

fn mux_route(args: &[&str]) -> Output {
    Bin::new(env!("CARGO_BIN_EXE_mux-route"))
        .args(args)
        .output()
}

#[test]
fn a_machine_that_is_not_one_of_the_two_is_refused() {
    let output = mux_route(&["nowhere"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let refused = stderr(&output);
    assert!(refused.contains("invalid value 'nowhere'"), "{refused}");
    assert!(refused.contains("macie"), "{refused}");
    assert!(refused.contains("archie"), "{refused}");
}

#[test]
fn this_machine_is_refused_before_anything_is_probed() {
    let this = Host::this().expect("a known machine");
    let output = mux_route(&[this.name()]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("this machine"), "{output:?}");
}

#[test]
fn the_completions_flag_answers_for_this_tool() {
    let output = mux_route(&["--completions", "zsh"]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("#compdef mux-route"), "{output:?}");
}

#[test]
fn the_completion_offers_the_machines_rather_than_file_names() {
    let output = mux_route(&["--completions", "zsh"]);
    let script = stdout(&output);
    assert!(script.contains("(macie archie)"), "{script}");
    assert!(
        !script.contains("host -- Machine to reach:_default"),
        "{script}"
    );
}

#[test]
fn one_host_at_a_time() {
    assert_eq!(mux_route(&["macie", "archie"]).status.code(), Some(2));
}
