use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use clap::Args;

#[derive(Args)]
pub struct CatFlags {
    #[arg(
        short = 'A',
        long = "show-all",
        help = "Show nonprinting characters, tabs and line ends"
    )]
    show_all: bool,

    #[arg(short = 'b', long = "number-nonblank", help = "Number nonempty lines")]
    number_nonblank: bool,

    #[arg(short = 'e', help = "Same as -vE")]
    ends_and_nonprinting: bool,

    #[arg(short = 'E', long = "show-ends", help = "Show $ at line ends")]
    show_ends: bool,

    #[arg(short = 'l', help = "Lock stdout")]
    lock: bool,

    #[arg(short = 'n', long = "number", help = "Number all lines")]
    number: bool,

    #[arg(
        short = 's',
        long = "squeeze-blank",
        help = "Squeeze repeated empty lines"
    )]
    squeeze_blank: bool,

    #[arg(short = 't', help = "Same as -vT")]
    tabs_and_nonprinting: bool,

    #[arg(short = 'T', long = "show-tabs", help = "Show tabs as ^I")]
    show_tabs: bool,

    #[arg(short = 'u', help = "Unbuffered output")]
    unbuffered: bool,

    #[arg(
        short = 'v',
        long = "show-nonprinting",
        help = "Show nonprinting characters"
    )]
    show_nonprinting: bool,
}

impl CatFlags {
    pub fn letters(&self) -> String {
        [
            (self.show_all, 'A'),
            (self.number_nonblank, 'b'),
            (self.ends_and_nonprinting, 'e'),
            (self.show_ends, 'E'),
            (self.lock, 'l'),
            (self.number, 'n'),
            (self.squeeze_blank, 's'),
            (self.tabs_and_nonprinting, 't'),
            (self.show_tabs, 'T'),
            (self.unbuffered, 'u'),
            (self.show_nonprinting, 'v'),
        ]
        .into_iter()
        .filter_map(|(set, letter)| set.then_some(letter))
        .collect()
    }

    // None when a flag needs cat itself; `-u` is ignored, as bat does.
    pub fn highlight(&self) -> Option<Highlight> {
        let cat_only = [
            self.number_nonblank,
            self.ends_and_nonprinting,
            self.show_ends,
            self.lock,
            self.tabs_and_nonprinting,
            self.show_tabs,
            self.show_nonprinting,
        ];
        (!cat_only.contains(&true)).then_some(Highlight {
            show_all: self.show_all,
            number: self.number,
            squeeze_blank: self.squeeze_blank,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    pub show_all: bool,
    pub number: bool,
    pub squeeze_blank: bool,
}

pub fn cat(letters: &str, files: &[PathBuf]) -> Command {
    let mut command = Command::new("cat");
    command.args(arguments(letters, files));
    command
}

fn arguments(letters: &str, files: &[PathBuf]) -> Vec<OsString> {
    let flags = (!letters.is_empty()).then(|| format!("-{letters}").into());
    flags
        .into_iter()
        .chain([OsString::from("--")])
        .chain(files.iter().map(|file| file.into()))
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/cat_tests.rs"]
mod tests;
