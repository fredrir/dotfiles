#![forbid(unsafe_code)]

use clap::{Arg, ArgAction, Command};
use workstation::surface;

#[test]
fn schema_roundtrip_retains_choices_aliases_conflicts_and_path_hints() {
    let mut parser = Command::new("example")
        .arg(
            Arg::new("mode")
                .long("mode")
                .visible_alias("style")
                .value_parser(["compact", "full"])
                .conflicts_with("quiet"),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .action(ArgAction::SetTrue),
        )
        .subcommand(
            Command::new("show")
                .visible_alias("ls")
                .arg(Arg::new("path").value_hint(clap::ValueHint::DirPath)),
        );
    parser.build();
    let encoded = serde_json::to_string(&surface::document(&parser, "example")).unwrap();
    let document: surface::Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(document.version, surface::VERSION);
    let mode = document
        .command
        .params
        .iter()
        .find(|p| p.name == "mode")
        .unwrap();
    assert_eq!(mode.choices, ["compact", "full"]);
    assert_eq!(mode.secondary, ["--style"]);
    assert!(mode.conflicts.contains(&"--quiet".into()));
    assert_eq!(document.command.children[0].aliases, ["ls"]);
    assert!(matches!(
        document.command.children[0].params[0].completion,
        Some(surface::Completion::Dirs)
    ));
}
