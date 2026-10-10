use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bat::assets::HighlightingAssets;
use bat::config::Config;
use bat::controller::Controller;
use bat::error::Error;
use bat::input::Input;
use bat::output::OutputHandle;
use bat::style::{StyleComponent, StyleComponents};
use bat::theme::{self, ThemeName, ThemeOptions, ThemePreference};
use bat::{BinaryBehavior, NonprintableNotation, StripAnsiMode, SyntaxMapping, WrappingMode};
use memchr::memchr;
use workstation::fail;

use crate::PROGRAM;
use crate::annotate::Annotator;
use crate::cat::Highlight;
use crate::filter::{self, CAPACITY, STDIN};

pub fn run(highlight: Highlight, files: &[PathBuf]) -> Result<ExitCode, String> {
    let config = config(highlight);
    let assets = HighlightingAssets::from_binary();
    let controller = Controller::new(&config, &assets);
    let mut output = Swatched::new(io::stdout().lock());
    let stdin = [PathBuf::from(STDIN)];
    let files = if files.is_empty() { &stdin[..] } else { files };
    let mut status = ExitCode::SUCCESS;
    for file in files {
        let mut failure = None;
        match open(file) {
            Ok((input, streamed)) => {
                output.flush_lines = streamed;
                let mut handle = OutputHandle::IoWrite(&mut output);
                controller
                    .run_with_error_handler(vec![input], Some(&mut handle), |error, _| {
                        failure = Some(describe(error));
                    })
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => failure = Some(filter::reason(&error)),
        }
        if let Some(error) = output.error.take() {
            return stopped(error, status);
        }
        if let Some(reason) = failure {
            if let Err(error) = output.flush() {
                return stopped(error, status);
            }
            status = fail(PROGRAM, format!("{}: {reason}", file.display()));
        }
    }
    match output.finish() {
        Ok(()) => Ok(status),
        Err(error) => stopped(error, status),
    }
}

fn config(highlight: Highlight) -> Config<'static> {
    let style = if highlight.number {
        StyleComponent::LineNumbers
    } else {
        StyleComponent::Plain
    };
    // bat wraps only decorated output, and only on a terminal.
    let wrapping_mode = if highlight.number && io::stdout().is_terminal() {
        WrappingMode::Character
    } else {
        WrappingMode::NoWrapping(false)
    };
    Config {
        language: highlight.show_all.then_some("show-nonprintable"),
        show_nonprintable: highlight.show_all,
        nonprintable_notation: NonprintableNotation::Unicode,
        binary: BinaryBehavior::NoPrinting,
        term_width: workstation::terminal_width().unwrap_or(80),
        tab_width: 4,
        colored_output: true,
        true_color: truecolor(),
        style_components: StyleComponents::new(&[style]),
        wrapping_mode,
        theme: theme::theme(theme_options()).to_string(),
        syntax_mapping: SyntaxMapping::new(),
        squeeze_lines: highlight.squeeze_blank.then_some(1),
        strip_ansi: StripAnsiMode::Never,
        ..Config::default()
    }
}

fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|value| value == "truecolor" || value == "24bit")
}

fn theme_options() -> ThemeOptions {
    let var = |key| std::env::var(key).ok();
    ThemeOptions {
        theme: var(theme::env::BAT_THEME)
            .map(ThemePreference::new)
            .unwrap_or_default(),
        theme_dark: var(theme::env::BAT_THEME_DARK).map(ThemeName::new),
        theme_light: var(theme::env::BAT_THEME_LIGHT).map(ThemeName::new),
    }
}

// The input, and whether it may stream: then every line is flushed as it comes.
fn open(file: &Path) -> io::Result<(Input<'static>, bool)> {
    if file.as_os_str() == STDIN {
        let stdin = File::from(io::stdin().as_fd().try_clone_to_owned()?);
        return Ok((Input::stdin(), !stdin.metadata()?.is_file()));
    }
    let opened = File::open(file)?;
    let metadata = opened.metadata()?;
    if metadata.is_dir() {
        return Err(io::ErrorKind::IsADirectory.into());
    }
    let mut input = Input::from_reader(Box::new(opened)).with_name(Some(file));
    input.description_mut().set_kind(Some("File".to_owned()));
    Ok((input, !metadata.is_file()))
}

fn describe(error: &Error) -> String {
    match error {
        Error::Io(error) => filter::reason(error),
        error => error.to_string(),
    }
}

fn stopped(error: io::Error, status: ExitCode) -> Result<ExitCode, String> {
    match error.kind() {
        io::ErrorKind::BrokenPipe => Ok(status),
        _ => Err(filter::reason(&error)),
    }
}

// Adds swatches to bat's output one line at a time as it is written.
struct Swatched<W: Write> {
    annotator: Annotator,
    output: BufWriter<W>,
    line: Vec<u8>,
    flush_lines: bool,
    error: Option<io::Error>,
}

impl<W: Write> Swatched<W> {
    fn new(output: W) -> Self {
        Self {
            annotator: Annotator::default(),
            output: BufWriter::with_capacity(CAPACITY, output),
            line: Vec::new(),
            flush_lines: false,
            error: None,
        }
    }

    fn swatch(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while let Some(end) = memchr(b'\n', bytes) {
            let (line, rest) = bytes.split_at(end + 1);
            self.line.extend_from_slice(line);
            self.annotator.annotate(&self.line, &mut self.output)?;
            self.line.clear();
            if self.flush_lines {
                self.output.flush()?;
            }
            bytes = rest;
        }
        self.line.extend_from_slice(bytes);
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        self.annotator.annotate(&self.line, &mut self.output)?;
        self.output.flush()
    }
}

impl<W: Write> Write for Swatched<W> {
    // bat only needs the kind to stop; the full error is kept for the report.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.swatch(bytes) {
            Ok(()) => Ok(bytes.len()),
            Err(error) => {
                let kind = error.kind();
                self.error.get_or_insert(error);
                Err(kind.into())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}
