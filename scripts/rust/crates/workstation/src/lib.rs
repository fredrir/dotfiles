//! Shared command-line plumbing for the workstation tools.
//!
//! Every binary in this workspace agrees on two things: a failure is one
//! `program: message` line on stderr and a non-zero status, and
//! `--completions <shell>` prints a completion script instead of doing the
//! tool's work. Both live here so a new tool inherits the conventions —
//! including the shell wiring in `shared/zsh/conf.d/55-completions.zsh`,
//! which assumes every tool answers the same flag.
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
}

impl Completions {
    /// `Some(status)` when the flag was given, for `main` to return straight
    /// away; `None` when the tool should get on with its actual work.
    pub fn emit<C: CommandFactory>(&self, program: &str) -> Option<ExitCode> {
        let shell = self.shell?;
        clap_complete::generate(shell, &mut C::command(), program, &mut io::stdout());
        Some(ExitCode::SUCCESS)
    }
}

/// Report a failure the way every tool here reports one.
pub fn fail(program: &str, message: impl Display) -> ExitCode {
    eprintln!("{program}: {message}");
    ExitCode::FAILURE
}

/// The palette `dotfile theme` exports, and the decision to use it.
///
/// Colour is for a person reading a terminal, so it is left out when stdout is
/// a pipe or when `NO_COLOR` asks. Each method takes the finished text and
/// hands it back either painted or untouched, which keeps padding — always
/// measured on the text, never on the escapes — at the call site.
pub struct Style {
    colored: bool,
    green: String,
    red: String,
    teal: String,
}

impl Style {
    /// The style for writing to stdout right now.
    pub fn for_stdout() -> Style {
        Style {
            colored: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
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
