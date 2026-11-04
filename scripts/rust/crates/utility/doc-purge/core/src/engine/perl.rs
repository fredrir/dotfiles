//! Perl: a dedicated scanner, because four Perl forms all look like a `#`
//! comment to a table-driven lexer.
//!
//! * `$#array` and `$#{$ref}` are the last-index sigil, not a comment.
//! * POD (`=pod`, `=head1`, …) is a comment from a directive at column 0 to the
//!   `=cut` line, inclusive. `=pod` inside a string is text.
//! * `__END__` and `__DATA__` at column 0 end Perl; a `#` after them is data
//!   and must survive.
//! * Quote-like operators (`q qq qw qr m s tr y`) and heredocs own their body,
//!   so a `#` there is text. `s`, `tr` and `y` take two delimiter pairs.
//!
//! The only ambiguous byte is `/`. It opens a regex exactly where the task's
//! allow-list says an expression may start; everywhere else it is division.
//! Bare regexes are kept to one line: an unclosed one falls back to division
//! rather than eating the rest of the file.

use std::ops::Range;

use crate::scan::{Bail, Out, char_width, ends_line, identifier_byte, line_end, starts_with};
use crate::syntax::Syntax;

const BOM: &[u8] = b"\xef\xbb\xbf";

/// Lex `source` with Perl's descriptor.
pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    match syntax.name {
        "perl" => Perl::new(source).run(),
        // `Engine::Perl` is only reached through this descriptor. Anything else
        // is a wiring bug, and an empty scan would hide it.
        _ => Err(Bail("perl engine has no scanner for this descriptor")),
    }
}

/// The previous token, reduced to what decides regex versus division.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prior {
    /// The start of the file: a statement may start here.
    Start,
    /// `=~` or `!~`.
    RegexBinding,
    /// `=>`.
    FatArrow,
    /// A plain assignment `=`.
    Assign,
    /// `(`, `,`, `{` or `;`: an expression may start after it.
    Open,
    /// `return`, `and`, `or`, `not`, `if`, `unless`, `while`, `until`,
    /// `split`, `grep`, `map`.
    Keyword,
    /// A value: a following `/` is division.
    Value,
    /// `}`: a block may end here, so a `/` at the next line's head may be a
    /// statement-opening regex.
    CloseBrace,
    /// Anything else.
    Other,
}

struct Here {
    tag: Vec<u8>,
    /// `<<~`: the terminator may be indented.
    indent: bool,
    /// A double-quoted or unquoted heredoc interpolates and escapes.
    escapes: bool,
}

struct Perl<'a> {
    source: &'a [u8],
    at: usize,
    out: Out,
    heredocs: Vec<Here>,
    prior: Prior,
    /// Only whitespace has been seen on this line so far.
    line_head: bool,
}

impl<'a> Perl<'a> {
    fn new(source: &'a [u8]) -> Perl<'a> {
        Perl {
            source,
            at: if source.starts_with(BOM) {
                BOM.len()
            } else {
                0
            },
            out: Out::default(),
            heredocs: Vec::new(),
            prior: Prior::Start,
            line_head: true,
        }
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn run(mut self) -> Result<Out, Bail> {
        while self.at < self.source.len() {
            if self.line_start() {
                if self.end_marker() {
                    // Everything from here on is data, not Perl. Emitting no
                    // comment past this point is the whole reason for the check.
                    return Ok(self.out);
                }
                if pod_start(self.source, self.at) {
                    self.pod()?;
                    continue;
                }
            }
            let byte = self.byte(self.at);
            if ends_line(byte) {
                self.at = past_line(self.source, self.at);
                self.line_head = true;
                self.take_heredocs()?;
                continue;
            }
            if matches!(byte, b' ' | b'\t') {
                self.at += 1;
                continue;
            }
            if byte == b'#' && self.comment_allowed() {
                self.line_comment();
                continue;
            }
            match byte {
                b'\'' => {
                    self.quoted(b'\'', true)?;
                    continue;
                }
                b'"' => {
                    self.quoted(b'"', true)?;
                    continue;
                }
                b'`' => {
                    self.quoted(b'`', true)?;
                    continue;
                }
                _ => {}
            }
            if let Some(kind) = self.quote_like_at() {
                self.quote_like(kind)?;
                continue;
            }
            if byte == b'<' && starts_with(self.source, self.at, b"<<") && self.heredoc_head()? {
                continue;
            }
            if byte == b'/' && self.regex_allowed() && self.regex() {
                continue;
            }
            if identifier_byte(byte) {
                self.identifier();
                continue;
            }
            self.token(byte);
        }
        if !self.heredocs.is_empty() {
            // A `<<EOF` with no following line never reaches the newline path.
            self.take_heredocs()?;
        }
        Ok(self.out)
    }

    fn line_start(&self) -> bool {
        self.at == 0
            || (self.at == BOM.len() && self.source.starts_with(BOM))
            || ends_line(self.source[self.at - 1])
    }

    /// `__END__` or `__DATA__` as a word at column 0. `__END__foo` is an
    /// ordinary bareword; anything after the marker is the data section.
    fn end_marker(&self) -> bool {
        [b"__END__".as_slice(), b"__DATA__".as_slice()]
            .iter()
            .any(|marker| {
                starts_with(self.source, self.at, marker)
                    && self
                        .source
                        .get(self.at + marker.len())
                        .is_none_or(|byte| !identifier_byte(*byte))
            })
    }

    /// A POD block: the directive line through the `=cut` line, inclusive.
    fn pod(&mut self) -> Result<(), Bail> {
        let start = self.at;
        let mut line = start;
        loop {
            let end = line_end(self.source, line);
            if pod_cut(self.source, line) {
                self.out.comment(start..end, start + 1..end);
                self.at = end;
                self.line_head = false;
                return Ok(());
            }
            if end >= self.source.len() {
                return Err(Bail("unterminated POD"));
            }
            line = past_line(self.source, end);
        }
    }

    /// `#` is a comment everywhere except the `$#` sigil.
    fn comment_allowed(&self) -> bool {
        self.at == 0 || self.source[self.at - 1] != b'$'
    }

    fn line_comment(&mut self) {
        let start = self.at;
        let end = line_end(self.source, start + 1);
        self.out.comment(start..end, start + 1..end);
        self.at = end;
        self.line_head = false;
    }

    fn quoted(&mut self, quote: u8, escapes: bool) -> Result<(), Bail> {
        let start = self.at + 1;
        let mut at = start;
        while at < self.source.len() {
            if self.source[at] == b'\\' {
                at += 1;
                if at < self.source.len() {
                    at += char_width(self.source, at);
                }
                continue;
            }
            if self.source[at] == quote {
                self.out.text(start..at, escapes, true, quote);
                self.at = at + 1;
                self.prior = Prior::Value;
                self.line_head = false;
                return Ok(());
            }
            at += char_width(self.source, at);
        }
        Err(Bail("unterminated string"))
    }

    /// A quote-like operator whose delimiter starts immediately after the word.
    fn quote_like_at(&self) -> Option<QuoteLike> {
        if self.at > 0 {
            let prev = self.source[self.at - 1];
            if identifier_byte(prev)
                || matches!(prev, b'$' | b'@' | b'%' | b'&' | b'-' | b':' | b'>')
            {
                return None;
            }
        }
        for kind in [
            QuoteLike::Tr,
            QuoteLike::Qq,
            QuoteLike::Qw,
            QuoteLike::Qr,
            QuoteLike::Q,
            QuoteLike::M,
            QuoteLike::S,
            QuoteLike::Y,
        ] {
            let word = kind.word();
            if !starts_with(self.source, self.at, word) {
                continue;
            }
            let open = self.at + word.len();
            if self
                .source
                .get(open)
                .is_some_and(|byte| byte.is_ascii_punctuation())
            {
                return Some(kind);
            }
        }
        None
    }

    fn quote_like(&mut self, kind: QuoteLike) -> Result<(), Bail> {
        let open_at = self.at + kind.word().len();
        let first = self.chunk(open_at, kind.regex())?;
        if kind.text() {
            self.out.text(first.body.clone(), true, true, first.close);
        }
        let end = if kind.two() {
            let (open, start) = if first.bracketed {
                let open = *self
                    .source
                    .get(first.end)
                    .ok_or(Bail("unterminated quote-like literal"))?;
                if !open.is_ascii_punctuation() {
                    return Err(Bail("unterminated quote-like literal"));
                }
                (open, first.end + 1)
            } else {
                // `s/a/b/` and `tr/ab/cd/`: the middle delimiter both closes
                // the first body and opens the second.
                (first.open, first.end)
            };
            self.chunk_from(open, start, false)?.end
        } else {
            first.end
        };
        let mut at = end;
        while at < self.source.len() && self.source[at].is_ascii_alphabetic() {
            at += 1;
        }
        self.at = at;
        self.prior = Prior::Value;
        self.line_head = false;
        Ok(())
    }

    fn chunk(&self, open_at: usize, regex: bool) -> Result<Chunk, Bail> {
        let open = *self
            .source
            .get(open_at)
            .ok_or(Bail("unterminated quote-like literal"))?;
        self.chunk_from(open, open_at + 1, regex)
    }

    /// Scan one delimited body. Bracket pairs nest; a backslash escapes the
    /// next byte. For a `/`-delimited regex, `/` inside `[...]` does not close.
    fn chunk_from(&self, open: u8, start: usize, regex: bool) -> Result<Chunk, Bail> {
        let close = closing(open);
        let bracketed = close != open;
        let mut cursor = start;
        let mut depth = 1usize;
        let mut class = false;
        while cursor < self.source.len() {
            let byte = self.source[cursor];
            if byte == b'\\' {
                cursor += 1;
                if cursor < self.source.len() {
                    cursor += char_width(self.source, cursor);
                }
                continue;
            }
            if regex && open == b'/' {
                if byte == b'[' && !class {
                    class = true;
                    cursor += 1;
                    continue;
                }
                if byte == b']' && class {
                    class = false;
                    cursor += 1;
                    continue;
                }
                if byte == b'/' && !class {
                    return Ok(Chunk {
                        body: start..cursor,
                        end: cursor + 1,
                        open,
                        close,
                        bracketed,
                    });
                }
                cursor += char_width(self.source, cursor);
                continue;
            }
            if bracketed && byte == open {
                depth += 1;
                cursor += char_width(self.source, cursor);
                continue;
            }
            if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(Chunk {
                        body: start..cursor,
                        end: cursor + 1,
                        open,
                        close,
                        bracketed,
                    });
                }
            }
            cursor += char_width(self.source, cursor);
        }
        Err(Bail("unterminated quote-like literal"))
    }

    /// `<<TAG`, `<<"TAG"`, `<<'TAG'` and `<<~TAG`.
    fn heredoc_head(&mut self) -> Result<bool, Bail> {
        let mut cursor = self.at + 2;
        let indent = self.byte(cursor) == b'~';
        if indent {
            cursor += 1;
        }
        let (tag, escapes, end) = match self.source.get(cursor).copied() {
            Some(quote @ (b'\'' | b'"')) => {
                let start = cursor + 1;
                let mut at = start;
                while at < self.source.len() && self.source[at] != quote {
                    at += char_width(self.source, at);
                }
                if at >= self.source.len() {
                    return Err(Bail("unterminated heredoc tag"));
                }
                (self.source[start..at].to_vec(), quote == b'"', at + 1)
            }
            Some(byte) if tag_start(byte) => {
                let start = cursor;
                let mut at = cursor;
                while at < self.source.len() && identifier_byte(self.source[at]) {
                    at += char_width(self.source, at);
                }
                (self.source[start..at].to_vec(), true, at)
            }
            _ => return Ok(false),
        };
        if tag.is_empty() {
            return Ok(false);
        }
        self.heredocs.push(Here {
            tag,
            indent,
            escapes,
        });
        self.at = end;
        self.prior = Prior::Value;
        self.line_head = false;
        Ok(true)
    }

    /// Consume the bodies of every heredoc opened on the line just ended, in
    /// opening order. The next body starts after the previous terminator line.
    fn take_heredocs(&mut self) -> Result<(), Bail> {
        let queued = std::mem::take(&mut self.heredocs);
        for here in queued {
            let body = self.at;
            loop {
                let begin = self.at;
                let end = line_end(self.source, begin);
                let mut probe = begin;
                if here.indent {
                    while probe < end && matches!(self.source[probe], b' ' | b'\t') {
                        probe += 1;
                    }
                }
                if self.source[probe..end] == here.tag[..] {
                    self.out.text(body..begin, here.escapes, true, 0);
                    self.at = if end < self.source.len() {
                        past_line(self.source, end)
                    } else {
                        end
                    };
                    break;
                }
                if end >= self.source.len() {
                    return Err(Bail("unterminated heredoc"));
                }
                self.at = past_line(self.source, end);
            }
        }
        self.line_head = true;
        Ok(())
    }

    fn regex_allowed(&self) -> bool {
        matches!(
            self.prior,
            Prior::Start
                | Prior::RegexBinding
                | Prior::FatArrow
                | Prior::Assign
                | Prior::Open
                | Prior::Keyword
        ) || (self.line_head && self.prior == Prior::CloseBrace)
    }

    /// A bare `/…/`. Returns `false` without consuming anything when the line
    /// ends first, so the caller can read the same `/` as division.
    fn regex(&mut self) -> bool {
        let start = self.at + 1;
        let mut cursor = start;
        let mut class = false;
        while cursor < self.source.len() {
            match self.source[cursor] {
                b'\\' => {
                    cursor += 1;
                    if cursor < self.source.len() {
                        cursor += char_width(self.source, cursor);
                    }
                }
                b'[' if !class => {
                    class = true;
                    cursor += 1;
                }
                b']' if class => {
                    class = false;
                    cursor += 1;
                }
                b'/' if !class => {
                    cursor += 1;
                    while cursor < self.source.len() && self.source[cursor].is_ascii_alphabetic() {
                        cursor += 1;
                    }
                    self.at = cursor;
                    self.prior = Prior::Value;
                    self.line_head = false;
                    return true;
                }
                b'\n' | b'\r' => return false,
                _ => cursor += char_width(self.source, cursor),
            }
        }
        false
    }

    fn identifier(&mut self) {
        let start = self.at;
        while self.at < self.source.len() && identifier_byte(self.source[self.at]) {
            self.at += char_width(self.source, self.at);
        }
        let word = &self.source[start..self.at];
        self.prior = if regex_keyword(word) {
            Prior::Keyword
        } else {
            Prior::Value
        };
        self.line_head = false;
    }

    fn token(&mut self, byte: u8) {
        if starts_with(self.source, self.at, b"=~") || starts_with(self.source, self.at, b"!~") {
            self.prior = Prior::RegexBinding;
            self.at += 2;
        } else if starts_with(self.source, self.at, b"=>") {
            self.prior = Prior::FatArrow;
            self.at += 2;
        } else if starts_with(self.source, self.at, b"==")
            || starts_with(self.source, self.at, b"!=")
            || starts_with(self.source, self.at, b"<=")
            || starts_with(self.source, self.at, b">=")
            || starts_with(self.source, self.at, b"++")
            || starts_with(self.source, self.at, b"--")
        {
            self.prior = Prior::Other;
            self.at += 2;
        } else {
            self.prior = match byte {
                b'(' | b',' | b'{' | b';' => Prior::Open,
                b'}' => Prior::CloseBrace,
                b'=' => Prior::Assign,
                _ => Prior::Other,
            };
            self.at += char_width(self.source, self.at);
        }
        self.line_head = false;
    }
}

struct Chunk {
    body: Range<usize>,
    end: usize,
    open: u8,
    close: u8,
    bracketed: bool,
}

#[derive(Clone, Copy)]
enum QuoteLike {
    Q,
    Qq,
    Qw,
    Qr,
    M,
    S,
    Tr,
    Y,
}

impl QuoteLike {
    fn word(self) -> &'static [u8] {
        match self {
            QuoteLike::Q => b"q",
            QuoteLike::Qq => b"qq",
            QuoteLike::Qw => b"qw",
            QuoteLike::Qr => b"qr",
            QuoteLike::M => b"m",
            QuoteLike::S => b"s",
            QuoteLike::Tr => b"tr",
            QuoteLike::Y => b"y",
        }
    }

    /// `s`, `tr` and `y` carry two delimited bodies.
    fn two(self) -> bool {
        matches!(self, QuoteLike::S | QuoteLike::Tr | QuoteLike::Y)
    }

    /// The first body is a pattern: `/` in a class does not close it.
    fn regex(self) -> bool {
        matches!(self, QuoteLike::Qr | QuoteLike::M | QuoteLike::S)
    }

    /// `q`, `qq` and `qw` are strings for glyph sweeping; regex forms are not.
    fn text(self) -> bool {
        matches!(self, QuoteLike::Q | QuoteLike::Qq | QuoteLike::Qw)
    }
}

fn closing(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        byte => byte,
    }
}

fn tag_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

/// `=word` at column 0 starts POD.
fn pod_start(source: &[u8], at: usize) -> bool {
    source.get(at) == Some(&b'=')
        && source
            .get(at + 1)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
}

/// `=cut` at column 0, ending at a non-word byte.
fn pod_cut(source: &[u8], line: usize) -> bool {
    starts_with(source, line, b"=cut")
        && source
            .get(line + 4)
            .is_none_or(|byte| !identifier_byte(*byte))
}

fn regex_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"return"
            | b"and"
            | b"or"
            | b"not"
            | b"if"
            | b"unless"
            | b"while"
            | b"until"
            | b"split"
            | b"grep"
            | b"map"
    )
}

/// The byte just past the line terminator starting at `end`.
fn past_line(source: &[u8], end: usize) -> usize {
    if end >= source.len() {
        return end;
    }
    if source[end] == b'\r' && source.get(end + 1) == Some(&b'\n') {
        end + 2
    } else {
        end + 1
    }
}

#[cfg(test)]
mod tests {
    use super::scan;
    use crate::scan::Out;
    use crate::syntax::hash::PERL;

    fn scan_text(source: &str) -> Out {
        scan(source.as_bytes(), &PERL).expect("source scans")
    }

    fn comment_spans(source: &str) -> Vec<(usize, usize)> {
        scan_text(source)
            .comments
            .into_iter()
            .map(|comment| (comment.span.start, comment.span.end))
            .collect()
    }

    #[test]
    fn hash_comment_goes() {
        let source = "my $x = 1; # note\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        let start = source.find('#').expect("marker");
        assert_eq!(out.comments[0].span, start..source.len() - 1);
        assert_eq!(&source[out.comments[0].body.clone()], " note");
    }

    #[test]
    fn last_index_sigil_is_not_a_comment() {
        assert!(comment_spans("my $last = $#array;\n").is_empty());
    }

    #[test]
    fn last_index_of_reference_is_not_a_comment() {
        assert!(comment_spans("my $last = $#{$ref};\n").is_empty());
    }

    #[test]
    fn shebang_is_reported_as_a_comment() {
        let source = "#!/usr/bin/perl\nprint 1;\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(out.comments[0].span, 0..15);
        assert_eq!(&source[out.comments[0].body.clone()], "!/usr/bin/perl");
    }

    #[test]
    fn pod_block_goes() {
        let source = "=pod\n\ntext\n\n=cut\nprint 1;\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(out.comments[0].span, 0..16);
        assert_eq!(&source[out.comments[0].body.clone()], "pod\n\ntext\n\n=cut");
    }

    #[test]
    fn pod_inside_a_string_survives() {
        let source = "my $s = '=pod';\n";
        let out = scan_text(source);
        assert!(out.comments.is_empty());
        assert_eq!(out.texts.len(), 1);
        assert_eq!(&source[out.texts[0].body.clone()], "=pod");
    }

    #[test]
    fn end_stops_scanning() {
        let source = "my $x = 1; # first\n__END__\n# data\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(
            out.comments[0].span.start,
            source.find('#').expect("marker")
        );
    }

    #[test]
    fn data_after_a_marker_prefix_is_not_stopped() {
        let source = "__END__foo; # note\n";
        assert_eq!(comment_spans(source).len(), 1);
    }

    #[test]
    fn end_marker_with_a_trailing_comment_still_stops() {
        assert!(comment_spans("__END__ # data\n# more data\n").is_empty());
    }

    #[test]
    fn data_stops_scanning() {
        let source = "my $x = 1; # first\n__DATA__\n# data\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(
            out.comments[0].span.start,
            source.find('#').expect("marker")
        );
    }

    #[test]
    fn hash_inside_q_is_text() {
        assert!(comment_spans("my $x = q(# not a comment);\n").is_empty());
    }

    #[test]
    fn hash_inside_qq_is_text() {
        assert!(comment_spans("my $x = qq(# not a comment);\n").is_empty());
    }

    #[test]
    fn hash_inside_qw_is_text() {
        assert!(comment_spans("my @x = qw(# not a comment);\n").is_empty());
    }

    #[test]
    fn hash_inside_match_is_text() {
        assert!(comment_spans("$x =~ m/a#b/;\n").is_empty());
    }

    #[test]
    fn hash_inside_substitution_is_text() {
        assert!(comment_spans("$x =~ s/a#b/c#d/g;\n").is_empty());
    }

    #[test]
    fn substitution_and_transliteration_pair_delimiters() {
        let source = "$x =~ s{a#b}{c#d};\n$y =~ tr[a#b][c#d];\n";
        assert!(comment_spans(source).is_empty());
    }

    #[test]
    fn quoted_heredoc_body_is_text() {
        let source = "my $x = <<'EOF';\n# body\nEOF\nmy $y = 1;\n";
        let out = scan_text(source);
        assert!(out.comments.is_empty());
        assert_eq!(out.texts.len(), 1);
        assert_eq!(&source[out.texts[0].body.clone()], "# body\n");
    }

    #[test]
    fn indented_heredoc_terminator() {
        let source = "my $x = <<~EOF;\n  # body\n  EOF\nmy $y = 1;\n";
        let out = scan_text(source);
        assert!(out.comments.is_empty());
        assert_eq!(out.texts.len(), 1);
        assert_eq!(&source[out.texts[0].body.clone()], "  # body\n");
    }

    #[test]
    fn hash_inside_heredoc_survives() {
        let source = "my $x = <<EOF;\n# body\nEOF\nmy $y = 1; # after\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(
            out.comments[0].span.start,
            source.rfind('#').expect("trailing comment")
        );
    }

    #[test]
    fn regex_at_statement_start_is_a_regex() {
        assert!(comment_spans("/a#b/;\n").is_empty());
    }

    #[test]
    fn multiple_heredocs_are_consumed_in_order() {
        let source = "print <<A, <<'B';\n# body a\nA\n# body b\nB\nprint 1; # after\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(
            out.comments[0].span.start,
            source.rfind('#').expect("trailing comment")
        );
        assert_eq!(out.texts.len(), 2);
        assert_eq!(&source[out.texts[0].body.clone()], "# body a\n");
        assert_eq!(&source[out.texts[1].body.clone()], "# body b\n");
    }

    #[test]
    fn double_quoted_heredoc_body_is_text() {
        let source = "my $x = <<\"EOF\";\n# body\nEOF\n";
        let out = scan_text(source);
        assert!(out.comments.is_empty());
        assert_eq!(out.texts.len(), 1);
        assert_eq!(&source[out.texts[0].body.clone()], "# body\n");
    }

    #[test]
    fn regex_after_a_block_at_line_start_is_a_regex() {
        let source = "if ($x) {\n    print 1;\n}\n/foo#bar/;\n";
        assert!(comment_spans(source).is_empty());
    }

    #[test]
    fn division_is_not_a_regex() {
        let source = "my $x = $a / $b; # note\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(
            out.comments[0].span.start,
            source.find('#').expect("marker")
        );
    }

    #[test]
    fn regex_after_binding_operator_is_a_regex() {
        let source = "$x =~ /a#b/; # note\n";
        let out = scan_text(source);
        assert_eq!(out.comments.len(), 1);
        assert_eq!(
            out.comments[0].span.start,
            source.rfind('#').expect("marker")
        );
    }

    #[test]
    fn unterminated_pod_bails() {
        match scan(b"=pod\ntext\n", &PERL) {
            Err(bail) => assert!(bail.0.starts_with("unterminated"), "{bail:?}"),
            Ok(_) => panic!("unterminated POD was accepted"),
        }
    }
}
