use std::process::{Command, Output};

fn agent_hop(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-hop"))
        .args(args)
        .output()
        .expect("agent-hop runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_describes_the_command_and_its_examples() {
    let output = agent_hop(&["--help"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(text.starts_with("Move a Codex or Claude Code CLI session to the other workstation"));
    assert!(text.contains("Usage: agent-hop"));
    assert!(text.contains("agent-hop codex SESSION_ID"));
    assert!(text.contains("[possible values: codex, claude]"));
}

#[test]
fn version_names_the_binary() {
    let output = agent_hop(&["--version"]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).starts_with("agent-hop "));
}

#[test]
fn zsh_completions_cover_the_public_surface() {
    let output = agent_hop(&["--completions", "zsh"]);
    assert!(output.status.success(), "{output:?}");
    let script = stdout(&output);
    assert!(script.starts_with("#compdef agent-hop\n"));
    for value in [
        "codex",
        "claude",
        "--dry-run",
        "--no-connect",
        "--color",
        "--completions",
    ] {
        assert!(script.contains(value), "completion script has no {value}");
    }
    assert!(script.contains("(auto always never)"));
}

#[test]
fn the_command_dump_describes_the_parser() {
    let output = agent_hop(&["--command-dump"]);
    assert!(output.status.success(), "{output:?}");
    let dump = stdout(&output);
    assert!(dump.starts_with(
        "C\tagent-hop\t0\tMove a Codex or Claude Code CLI session to the other workstation"
    ));
    assert!(dump.contains("\toption\tdry_run\t-n,--dry-run\t"));
    assert!(dump.contains("\toption\tno_connect\t--no-connect\t"));
    assert!(dump.contains("\toption\tcolor\t--color\tWHEN\t"));
    assert!(dump.contains("\targument\tagent\t\tAGENT\t"));
    assert!(dump.contains("\targument\tsession_id\t\tSESSION_ID\t"));
}

#[test]
fn a_missing_agent_is_a_usage_error() {
    let output = agent_hop(&[]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let complaint = stderr(&output);
    assert!(complaint.contains("required arguments were not provided"));
    assert!(complaint.contains("<AGENT>"));
}

#[test]
fn an_invalid_agent_is_a_usage_error() {
    let output = agent_hop(&["other"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let complaint = stderr(&output);
    assert!(complaint.contains("invalid value 'other'"));
    assert!(complaint.contains("codex, claude"));
}

#[test]
fn extra_arguments_are_a_usage_error() {
    let output = agent_hop(&["codex", "one", "two"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(stderr(&output).contains("unexpected argument 'two'"));
}

#[test]
fn every_color_mode_is_accepted() {
    for mode in ["auto", "always", "never"] {
        let output = agent_hop(&["--color", mode, "--help"]);
        assert!(output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn an_invalid_color_mode_is_a_usage_error() {
    let output = agent_hop(&["--color", "sometimes", "--help"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let complaint = stderr(&output);
    assert!(complaint.contains("invalid value 'sometimes'"));
    assert!(complaint.contains("auto, always, never"));
}
