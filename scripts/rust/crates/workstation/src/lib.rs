use std::fmt::Display;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{Args, CommandFactory};
use clap_complete::Shell;

pub mod screen;

pub use screen::{Key, Screen};

// The `--completions <SHELL>` flag, flattened into each tool's parser. A
// required positional has to opt out of being required when the flag is
// present, with `#[arg(required_unless_present = "shell")]`.
//
// Deliberately not a doc comment: clap hands a flattened struct's doc comment
// to the command it is flattened into, which would replace every tool's own
// `about` in `--help` with this.
#[derive(Args)]
pub struct Completions {
    #[arg(long = "completions", value_name = "SHELL", exclusive = true)]
    pub shell: Option<Shell>,

    #[arg(long = "command-dump", exclusive = true, hide = true)]
    pub dump: bool,
}

impl Completions {
    pub fn is_zsh(&self) -> bool {
        self.shell == Some(Shell::Zsh)
    }

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

pub fn fail(program: &str, message: impl Display) -> ExitCode {
    eprintln!("{program}: {message}");
    ExitCode::FAILURE
}

pub struct Style {
    colored: bool,
    green: String,
    red: String,
    teal: String,
}

impl Style {
    pub fn for_stdout() -> Style {
        Style::for_stream(io::stdout().is_terminal())
    }

    pub fn for_stdout_with_color(colored: bool) -> Style {
        Style::new(colored)
    }

    pub fn for_stderr() -> Style {
        Style::for_stream(io::stderr().is_terminal())
    }

    fn for_stream(terminal: bool) -> Style {
        Self::new(terminal && std::env::var_os("NO_COLOR").is_none())
    }

    fn new(colored: bool) -> Style {
        Style {
            colored,
            green: theme("THEME_GIT", "\x1b[32m"),
            red: theme("THEME_SUDO", "\x1b[31m"),
            teal: theme("THEME_DIR", "\x1b[36m"),
        }
    }

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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    All,
}

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

pub fn terminal_width() -> Option<usize> {
    if let Some(columns) = std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()) {
        return Some(columns);
    }
    terminal_size().map(|(columns, _rows)| columns)
}

pub fn terminal_height() -> Option<usize> {
    if let Some(rows) = std::env::var("LINES").ok().and_then(|v| v.parse().ok()) {
        return Some(rows);
    }
    terminal_size().map(|(_columns, rows)| rows)
}

#[cfg(unix)]
fn terminal_size() -> Option<(usize, usize)> {
    // SAFETY: `winsize` is four integers, and the ioctl either fills them in
    // and reports success or leaves them alone and reports failure.
    let (ok, size) = unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        let status = libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size);
        (status == 0, size)
    };
    (ok && size.ws_col > 0).then_some((size.ws_col as usize, size.ws_row as usize))
}

#[cfg(not(unix))]
fn terminal_size() -> Option<(usize, usize)> {
    None
}

#[cfg(test)]
#[path = "../tests/unit/lib_tests.rs"]
mod tests;
