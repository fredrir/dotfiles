use std::io::{self, BufRead, Write};
use std::process::Command;

use ui_terminal::text::{fit, sanitize};
use ui_widgets::SearchIndex;

pub struct Row<'a> {
    pub label: &'a str,
    pub copy: &'a str,
}

pub struct Options<'a> {
    pub title: &'a str,
    pub header: &'a str,
    pub colors: &'a str,
    pub copy_command: &'a str,
    pub popup: bool,
}

pub fn input(rows: &[Row<'_>]) -> String {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let copy = if row.copy.is_empty() {
                row.label
            } else {
                row.copy
            };
            format!("{index}\t{}\t{}", fit(copy, 1200), fit(row.label, 1200))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn command(options: &Options<'_>) -> Command {
    let mut command = Command::new("fzf");
    command
        .env("FZF_DEFAULT_OPTS", "")
        .env("FZF_DEFAULT_OPTS_FILE", "")
        .args([
            "--layout=reverse",
            "--no-multi",
            "--delimiter=\t",
            "--with-nth=3..",
            "--cycle",
        ]);
    if options.copy_command.is_empty() {
        command.args(["--header", &sanitize(options.header)]);
    } else {
        command.args([
            "--header",
            &format!("{} · ⌃y copy row", sanitize(options.header)),
            "--bind",
            &format!("ctrl-y:execute-silent({} {{2}})", options.copy_command),
        ]);
    }
    if options.popup {
        command.args(["--padding=0,1", "--prompt", "› "]);
    } else {
        command.args([
            "--border=rounded",
            "--prompt",
            &format!("{} › ", sanitize(options.title)),
        ]);
    }
    if !options.colors.is_empty() {
        command.args(["--color", options.colors]);
    }
    command
}

pub fn selection(code: i32, output: &str, count: usize) -> io::Result<Option<usize>> {
    match code {
        0 => output
            .split('\t')
            .next()
            .unwrap_or_default()
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|index| *index < count)
            .map(Some)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid picker result")),
        1 | 130 => Ok(None),
        _ => Err(io::Error::other(format!("picker exited {code}"))),
    }
}

pub fn choose_plain(
    title: &str,
    rows: &[Row<'_>],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<usize>> {
    writeln!(
        output,
        "{} — fzf unavailable; enter a number or search text\n",
        sanitize(title)
    )?;
    let index = SearchIndex::new(rows.iter().map(|row| row.label));
    let mut query = String::new();
    let mut visible = Vec::new();
    let mut answer = String::new();
    loop {
        index.filter_into(&query, &mut visible);
        for item in visible.iter().take(80) {
            writeln!(output, "{:>3}  {}", item + 1, fit(rows[*item].label, 180))?;
        }
        write!(output, "Number / search / empty to cancel › ")?;
        output.flush()?;
        answer.clear();
        if input.read_line(&mut answer)? == 0 || answer.trim().is_empty() {
            return Ok(None);
        }
        let value = answer.trim();
        if let Ok(number) = value.parse::<usize>()
            && (1..=rows.len()).contains(&number)
        {
            return Ok(Some(number - 1));
        }
        query.clear();
        query.push_str(value);
    }
}
