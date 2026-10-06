#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;
mod obsidian;
mod source;
mod tables;

use comrak::Options;

use config::Config;
use dialect::Dialect;

pub fn options(config: &Config) -> Options<'static> {
    let mut options = Options::default();
    if config.dialect != Dialect::Commonmark {
        options.extension.table = true;
        options.extension.strikethrough = true;
        options.extension.tasklist = true;
        options.extension.autolink = true;
        options.extension.footnotes = true;
        options.extension.front_matter_delimiter = Some("---".into());
        options.extension.alerts = config.dialect != Dialect::Obsidian;
    }
    if config.dialect == Dialect::Obsidian {
        options.extension.highlight = true;
        options.extension.inline_footnotes = true;
    }
    // Width only selects table alignment. Never reflow prose or list contents.
    options.render.width = 0;
    options.render.list_style = config.list_marker;
    options.render.prefer_fenced = true;
    options
}

pub fn format(input: &str, config: &Config) -> Result<String, String> {
    let input = input.replace("\r\n", "\n");
    let (frontmatter, body) = if config.dialect != Dialect::Commonmark {
        split_frontmatter(&input)
    } else {
        ("", input.as_str())
    };
    let options = options(config);
    let protected = (config.dialect != Dialect::Commonmark)
        .then(|| obsidian::Protected::new(body, &options, config.dialect == Dialect::Obsidian))
        .transpose()?;
    let text = protected.as_ref().map_or(body, |p| p.text.as_str());
    let formatted = source::format(text, config, &options, protected.as_ref())?;
    let mut output = frontmatter.to_owned();
    output.push_str(&match protected {
        Some(protected) => protected.restore(&formatted)?,
        None => formatted,
    });
    // The source formatter terminates every emitted line. Remove its added
    // terminator before deciding whether the original blank lines should stay.
    if !input.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }
    let trailing_blank_bytes: usize = output
        .split_inclusive('\n')
        .rev()
        .take_while(|line| line.trim_matches([' ', '\t', '\n']).is_empty())
        .map(str::len)
        .sum();
    if config.trim_trailing_blank_lines {
        output.truncate(output.len() - trailing_blank_bytes);
    }
    // Retained blank lines take precedence over removing the final newline;
    // otherwise each formatting pass would remove another empty line.
    if config.trim_trailing_blank_lines || trailing_blank_bytes == 0 {
        output.truncate(output.trim_end_matches('\n').len());
    }
    if config.final_newline && !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

// Recognize metadata before Markdown parsing, including empty YAML blocks and
// YAML's explicit document terminator. Keep every byte inside it untouched.
fn split_frontmatter(input: &str) -> (&str, &str) {
    let mut lines = input.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return ("", input);
    };
    let delimiter = first.trim_end_matches('\n');
    if delimiter != "---" && delimiter != "+++" {
        return ("", input);
    }
    let mut end = first.len();
    for line in lines {
        end += line.len();
        let value = line.trim_end_matches('\n');
        if value == delimiter || (delimiter == "---" && value == "...") {
            return input.split_at(end);
        }
    }
    ("", input)
}
