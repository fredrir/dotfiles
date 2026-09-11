use clap::{Parser, error::ErrorKind};
use ui_cli::{Answer, prompt, try_parse_from};

#[derive(Debug, Parser, PartialEq)]
#[command(name = "fixture")]
struct Cli {
    #[arg(long)]
    flag: bool,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Debug, clap::Subcommand, PartialEq)]
enum Action {
    Show { path: String },
}

#[test]
fn shared_presentation_preserves_parsing_and_nested_help() {
    assert_eq!(
        try_parse_from::<Cli, _, _>(["fixture", "--flag", "show", "a b"]).unwrap(),
        Cli {
            flag: true,
            action: Some(Action::Show { path: "a b".into() })
        }
    );
    let help = try_parse_from::<Cli, _, _>(["fixture", "show", "--help"]).unwrap_err();
    assert_eq!(help.kind(), ErrorKind::DisplayHelp);
    assert!(help.to_string().contains("fixture show"));
    assert!(help.to_string().contains("<PATH>"));
    let error = try_parse_from::<Cli, _, _>(["fixture", "--unknown"]).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    assert_eq!(error.exit_code(), 2);
}

#[test]
fn confirmation_reprompts_without_losing_default_or_eof_semantics() {
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let answer = prompt(
        &mut &b"invalid\nno\n"[..],
        &mut output,
        &mut errors,
        "Continue? ",
        false,
    )
    .unwrap();
    assert_eq!(answer, Some(Answer::No));
    assert_eq!(String::from_utf8(output).unwrap(), "Continue? Continue? ");
    assert_eq!(
        String::from_utf8(errors).unwrap(),
        "Please answer y or n.\n"
    );
    assert_eq!(
        prompt(
            &mut &b"\n"[..],
            &mut Vec::new(),
            &mut Vec::new(),
            "?",
            false
        )
        .unwrap(),
        Some(Answer::Yes)
    );
    assert_eq!(
        prompt(&mut &b""[..], &mut Vec::new(), &mut Vec::new(), "?", false).unwrap(),
        None
    );
    assert_eq!(
        prompt(
            &mut &b"all\n"[..],
            &mut Vec::new(),
            &mut Vec::new(),
            "?",
            true
        )
        .unwrap(),
        Some(Answer::All)
    );
}

#[test]
fn help_styles_honor_the_selected_terminal_color_depth() {
    let palette = ui_theme::Palette::from_json(
        r##"{"version":1,"profile":"test","colors":{"fg":"#ffffff"},"roles":{},"ui":{"accent":"#7854fa"}}"##,
    )
    .unwrap()
    .with_depth(ui_theme::ColorDepth::Ansi16);
    let header = ui_cli::styles(&palette).get_header().get_fg_color();
    assert!(
        matches!(header, Some(clap::builder::styling::Color::Ansi(_))),
        "{header:?}"
    );
}
