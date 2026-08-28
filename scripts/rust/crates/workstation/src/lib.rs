//! Shared command-line plumbing for the workstation tools.
//!
//! Every binary in this workspace agrees on two things: a failure is one
//! `program: message` line on stderr and a non-zero status, and
//! `--completions <shell>` prints a completion script instead of doing the
//! tool's work. Both live here so a new tool inherits the conventions —
//! including the shell wiring in `shared/zsh/conf.d/55-completions.zsh`,
//! which assumes every tool answers the same flag.
//!
//! `--command-dump` is the same idea aimed at `docs/cli`: it prints the
//! parser as lines rather than prose, so the tables in those pages are
//! generated from the parser instead of transcribed from `--help`.
//!
//! The terminal itself is the third thing they share: the same palette, the
//! same rule for when to use it, and the same question when something is
//! about to happen that cannot be taken back.

use std::fmt::Display;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{Args, CommandFactory};
use clap_complete::Shell;

// The `--completions <SHELL>` flag, flattened into each tool's parser. A
// required positional has to opt out of being required when the flag is
// present, with `#[arg(required_unless_present = "shell")]`.
//
// Deliberately not a doc comment: clap hands a flattened struct's doc comment
// to the command it is flattened into, which would replace every tool's own
// `about` in `--help` with this.
#[derive(Args)]
pub struct Completions {
    /// Print shell completions and exit
    #[arg(long = "completions", value_name = "SHELL", exclusive = true)]
    pub shell: Option<Shell>,

    /// Print the parser as lines, for `dotfile docs`
    #[arg(long = "command-dump", exclusive = true, hide = true)]
    pub dump: bool,
}

impl Completions {
    /// `Some(status)` when either flag was given, for `main` to return straight
    /// away; `None` when the tool should get on with its actual work.
    pub fn emit<C: CommandFactory>(&self, program: &str) -> Option<ExitCode> {
        if self.dump {
            let mut command = C::command();
            command.build();
            dump(&command, program);
            return Some(ExitCode::SUCCESS);
        }
        let shell = self.shell?;
        clap_complete::generate(shell, &mut C::command(), program, &mut io::stdout());
        Some(ExitCode::SUCCESS)
    }
}

/// One tab-separated line per command and per argument, parents first.
///
/// Tabs and newlines are the only characters the reader splits on, so help
/// text is flattened rather than quoted: a description that wrapped in the
/// source should read as one sentence in a table anyway.
fn dump(command: &clap::Command, path: &str) {
    println!(
        "C\t{path}\t{}\t{}",
        usize::from(command.is_hide_set()),
        flatten(command.get_about().map(|about| about.to_string()))
    );
    for argument in command.get_arguments() {
        let takes_value = argument.get_action().takes_values();
        let repeats = matches!(argument.get_action(), clap::ArgAction::Append)
            || argument
                .get_num_args()
                .is_some_and(|range| range.max_values() > 1);
        println!(
            "A\t{path}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            if argument.is_positional() {
                "argument"
            } else {
                "option"
            },
            argument.get_id(),
            spellings(argument),
            if takes_value {
                metavar(argument)
            } else {
                String::new()
            },
            usize::from(repeats),
            usize::from(argument.is_required_set()),
            usize::from(argument.is_hide_set()),
            flatten(argument.get_help().map(|help| help.to_string())),
        );
    }
    for child in command.get_subcommands() {
        dump(child, &format!("{path} {}", child.get_name()));
    }
}

fn spellings(argument: &clap::Arg) -> String {
    let mut found = Vec::new();
    if let Some(short) = argument.get_short() {
        found.push(format!("-{short}"));
    }
    if let Some(long) = argument.get_long() {
        found.push(format!("--{long}"));
    }
    found.join(",")
}

fn metavar(argument: &clap::Arg) -> String {
    if let Some(names) = argument.get_value_names()
        && let Some(first) = names.first()
    {
        return first.to_string();
    }
    argument.get_id().to_string().to_uppercase()
}

fn flatten(text: Option<String>) -> String {
    text.unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Report a failure the way every tool here reports one.
pub fn fail(program: &str, message: impl Display) -> ExitCode {
    eprintln!("{program}: {message}");
    ExitCode::FAILURE
}

/// The palette `dotfile theme` exports, and the decision to use it.
///
/// Colour is for a person reading a terminal, so it is left out when the stream
/// being written to is a pipe or when `NO_COLOR` asks. Each method takes the
/// finished text and hands it back either painted or untouched, which keeps
/// padding — always measured on the text, never on the escapes — at the call
/// site.
pub struct Style {
    colored: bool,
    green: String,
    red: String,
    teal: String,
}

impl Style {
    /// The style for writing to stdout right now.
    pub fn for_stdout() -> Style {
        Style::for_stream(io::stdout().is_terminal())
    }

    /// The style for writing to stderr right now.
    ///
    /// A tool whose stdout carries data rather than prose — `dotfmt --stdin`
    /// hands its result to an editor — says everything a person reads on
    /// stderr, so that is the stream whose terminal-ness decides the colour.
    pub fn for_stderr() -> Style {
        Style::for_stream(io::stderr().is_terminal())
    }

    fn for_stream(terminal: bool) -> Style {
        Style {
            colored: terminal && std::env::var_os("NO_COLOR").is_none(),
            green: theme("THEME_GIT", "\x1b[32m"),
            red: theme("THEME_SUDO", "\x1b[31m"),
            teal: theme("THEME_DIR", "\x1b[36m"),
        }
    }

    /// A style that paints nothing, for tests and for output that is data.
    pub fn plain() -> Style {
        Style {
            colored: false,
            green: String::new(),
            red: String::new(),
            teal: String::new(),
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint("\x1b[1m", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("\x1b[2m", text)
    }

    pub fn green(&self, text: &str) -> String {
        self.paint(&self.green, text)
    }

    pub fn red(&self, text: &str) -> String {
        self.paint(&self.red, text)
    }

    pub fn teal(&self, text: &str) -> String {
        self.paint(&self.teal, text)
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if !self.colored || text.is_empty() {
            return text.to_string();
        }
        format!("{code}{text}\x1b[0m")
    }
}

fn theme(name: &str, fallback: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => value,
        _ => fallback.to_string(),
    }
}

/// Ask `question` and read the answer: empty or `y` is yes, `n` is no, and
/// anything else asks again. `None` when the answers ran out before there was
/// one, which is a cancellation rather than a yes.
pub fn confirm(question: &str) -> Option<bool> {
    let mut answer = String::new();
    loop {
        print!("{question}");
        io::stdout().flush().ok()?;
        answer.clear();
        if io::stdin().read_line(&mut answer).ok()? == 0 {
            return None;
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Some(true),
            "n" | "no" => return Some(false),
            _ => eprintln!("Please answer y or n."),
        }
    }
}

/// One of the three answers to a question asked once per thing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    /// Yes to this one and to everything still to be asked about.
    All,
}

/// Ask `question` about one of several things: empty or `y` is yes, `n` is
/// no, `a` is yes to this and the rest, and anything else asks again. `None`
/// when the answers ran out before there was one, which is a cancellation
/// rather than a yes.
pub fn confirm_each(question: &str) -> Option<Answer> {
    let mut answer = String::new();
    loop {
        print!("{question}");
        io::stdout().flush().ok()?;
        answer.clear();
        if io::stdin().read_line(&mut answer).ok()? == 0 {
            return None;
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Some(Answer::Yes),
            "n" | "no" => return Some(Answer::No),
            "a" | "all" => return Some(Answer::All),
            _ => eprintln!("Please answer y, n or a."),
        }
    }
}

/// How wide the terminal is, when there is one to ask.
///
/// `COLUMNS` wins where it is set, which is both what a shell exports for its
/// children and how a test pins the width; otherwise the terminal is asked
/// directly, since a program is not told when it is resized.
pub fn terminal_width() -> Option<usize> {
    if let Some(columns) = std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()) {
        return Some(columns);
    }
    terminal_columns()
}

#[cfg(unix)]
fn terminal_columns() -> Option<usize> {
    // SAFETY: `winsize` is four integers, and the ioctl either fills them in
    // and reports success or leaves them alone and reports failure.
    let (ok, size) = unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        let status = libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size);
        (status == 0, size)
    };
    (ok && size.ws_col > 0).then_some(size.ws_col as usize)
}

#[cfg(not(unix))]
fn terminal_columns() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
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
}
