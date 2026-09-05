use super::*;

#[test]
fn a_plain_style_paints_nothing() {
    let style = Style::plain();
    assert_eq!(style.bold("gdd"), "gdd");
    assert_eq!(style.teal("~/dotfiles"), "~/dotfiles");
}

#[test]
fn painting_wraps_the_text_it_is_given() {
    let style = Style {
        colored: true,
        green: "\x1b[32m".into(),
        red: String::new(),
        teal: String::new(),
    };
    assert_eq!(style.green("+2"), "\x1b[32m+2\x1b[0m");
    assert_eq!(style.green(""), "");
}

#[test]
fn a_mode_decides_whether_a_style_paints() {
    assert_eq!(Style::for_mode(ColorMode::Never, true).bold("gdd"), "gdd");
    assert_eq!(
        Style::for_mode(ColorMode::Always, false).bold("gdd"),
        "\x1b[1mgdd\x1b[0m"
    );
    assert_eq!(Style::for_mode(ColorMode::Auto, false).dim("gdd"), "gdd");
}

#[test]
fn an_arbitrary_code_paints_the_way_the_named_colours_do() {
    let style = Style::for_mode(ColorMode::Always, false);
    assert_eq!(
        style.code("1;38;2;52;211;153", "up"),
        "\x1b[1;38;2;52;211;153mup\x1b[0m"
    );
    assert_eq!(style.code("2", "idle"), "\x1b[2midle\x1b[0m");
    assert_eq!(style.code("2", ""), "");
    assert_eq!(Style::plain().code("2", "idle"), "idle");
}

#[derive(clap::Parser)]
struct Nested {
    #[command(flatten)]
    completions: Completions,
}

#[derive(clap::Parser)]
struct Cli {
    #[command(flatten)]
    common: Nested,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.common.completions
    }
}

#[test]
fn a_completable_parser_hands_back_the_flag_it_flattened() {
    use clap::Parser;

    let asked = Cli::try_parse_from(["tool", "--completions", "zsh"]).expect("a parse");
    assert!(asked.completions().is_zsh());

    let bare = Cli::try_parse_from(["tool"]).expect("a parse");
    assert!(!bare.completions().is_zsh());
    assert!(!bare.completions().dump);
}

#[test]
fn the_run_helper_takes_any_completable_parser() {
    let never_called = |program: &str| run(program, |_cli: Cli| Ok(ExitCode::SUCCESS));
    let _ = never_called;
}
