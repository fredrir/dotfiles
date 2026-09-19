#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fmt::Display;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use clap::builder::styling::{Ansi256Color, Color, RgbColor, Style, Styles};
use clap::{Command, CommandFactory, Parser};
use ui_theme::{Palette, Role};

pub fn styles(palette: &Palette) -> Styles {
    let paint = |role| {
        Style::new().fg_color(match palette.foreground(role) {
            ui_theme::Color::Terminal => None,
            ui_theme::Color::Ansi(value) => Some(
                Ansi256Color(value)
                    .into_ansi()
                    .map(Color::Ansi)
                    .unwrap_or(Color::Ansi256(Ansi256Color(value))),
            ),
            ui_theme::Color::Rgb(red, green, blue) => Some(Color::Rgb(RgbColor(red, green, blue))),
        })
    };
    Styles::styled()
        .header(paint(Role::Accent).bold())
        .usage(paint(Role::Accent).bold())
        .literal(paint(Role::Strong).bold())
        .placeholder(paint(Role::Muted))
        .error(paint(Role::Danger).bold())
        .valid(paint(Role::Success))
        .invalid(paint(Role::Warning))
        .context(paint(Role::Muted))
}

pub fn decorate(command: Command) -> Command {
    decorate_with_palette(command, &Palette::current())
}

pub fn decorate_with_palette(mut command: Command, palette: &Palette) -> Command {
    command = command.styles(styles(palette)).max_term_width(100);
    // Explicit child styling also covers independently parsed subcommands.
    for child in command.get_subcommands_mut() {
        *child = decorate_with_palette(std::mem::take(child), palette);
    }
    command
}

pub fn command<C: CommandFactory>() -> Command {
    decorate(C::command())
}

pub fn try_parse_from<C, I, T>(arguments: I) -> Result<C, clap::Error>
where
    C: Parser,
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut command = command::<C>();
    let matches = command.try_get_matches_from_mut(arguments)?;
    C::from_arg_matches(&matches).map_err(|error| error.format(&mut command))
}

pub fn parse<C: Parser>() -> C {
    try_parse_from(std::env::args_os()).unwrap_or_else(|error| error.exit())
}

pub fn fail(program: &str, message: impl Display) -> ExitCode {
    eprintln!("{program}: {message}");
    ExitCode::FAILURE
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Answer {
    Yes,
    No,
    All,
}

pub fn confirm(question: &str) -> Option<bool> {
    let answer = prompt(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        question,
        false,
    )
    .ok()??;
    Some(answer == Answer::Yes)
}

pub fn confirm_each(question: &str) -> Option<Answer> {
    prompt(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        question,
        true,
    )
    .ok()?
}

/// Reads one line from the controlling terminal without echoing it.
pub fn hidden(question: &str) -> io::Result<String> {
    rpassword::prompt_password(question)
}

pub fn prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    errors: &mut impl Write,
    question: &str,
    allow_all: bool,
) -> io::Result<Option<Answer>> {
    let mut answer = String::new();
    loop {
        write!(output, "{question}")?;
        output.flush()?;
        answer.clear();
        if input.read_line(&mut answer)? == 0 {
            return Ok(None);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(Some(Answer::Yes)),
            "n" | "no" => return Ok(Some(Answer::No)),
            "a" | "all" if allow_all => return Ok(Some(Answer::All)),
            _ => writeln!(
                errors,
                "{}",
                if allow_all {
                    "Please answer y, n or a."
                } else {
                    "Please answer y or n."
                }
            )?,
        }
    }
}
