use std::borrow::Cow;

use crate::config::Config;

/// Comment text as written, optionally trimmed and with one space after `#`.
///
/// `#!` is left alone so a shebang-shaped first line keeps its meaning.
pub(crate) fn format<'a>(text: &'a str, config: &Config) -> Cow<'a, str> {
    let text = if config.trim_trailing_whitespace {
        text.trim_end_matches([' ', '\t'])
    } else {
        text
    };
    let Some(rest) = text.strip_prefix('#') else {
        return Cow::Borrowed(text);
    };
    if config.format_comments && !rest.is_empty() && !rest.starts_with([' ', '\t', '#', '!']) {
        Cow::Owned(format!("# {rest}"))
    } else {
        Cow::Borrowed(text)
    }
}
