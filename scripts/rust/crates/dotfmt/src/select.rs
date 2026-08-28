//! Which files a run is about: the `include` and `exclude` blocks.
//!
//! Both are .gitignore syntax, parsed and matched by git's own implementation
//! rather than by an approximation of it: `gix-glob` reads a pattern, hands
//! back git's `ABSOLUTE`/`MUST_BE_DIR`/`NO_SUB_DIR`/`NEGATIVE` flags, and
//! answers whether it matches a path through git's own wildmatch. That is the
//! whole reason it is here — the previous round hand-rolled this and got
//! `**/lua` wrong, and a selection rule that is *nearly* git's is a rule
//! nobody can predict. Only "the last entry to match wins, and a `!` one puts
//! the path back" is written out here, because that part is four lines and
//! `gix-ignore`, which does it, silently drops any pattern it cannot parse.
//!
//! `**` spans directories only when it stands alone between slashes, which is
//! git's rule and so gix's. `**ssh` is therefore just `*ssh` — a *single*
//! component ending in "ssh", which is `.ssh` and `openssh` as much as it is
//! `ssh`. The spelling that means what people mean is `ssh/_empty_`.
//!
//! An **`include` entry** ends in a token: `.conf`, `.config`, `.dotfile`, or
//! the literal `_empty_` for a name with no extension at all. The token says
//! which files the entry is about, because no glob can spell "has no
//! extension" and `.conf` as a plain pattern would match a file *named*
//! `.conf`. Everything before the token is an ordinary .gitignore pattern
//! naming the directory those files sit in, and a directory holds everything
//! below it — exactly what a `.gitignore` line naming a directory means. So
//! `**ssh/_empty_` picks up `linux/arch/ssh/config.d/40-cabled`: `**ssh`
//! matches the `ssh` directory, and the file is below it. A bare token has no
//! directory part at all and is shorthand for `**/<token>`: everywhere.
//!
//! An **`exclude` entry** has no token. It is a plain .gitignore pattern
//! matched against paths, applied the way git applies one: every directory on
//! the way down is tested, and a directory that matches takes everything below
//! it with it.
//!
//! Every path handed to this module is relative to the directory the patterns
//! were written in, which is what makes a leading `/` mean "here".

use std::path::Path;

use bstr::{BStr, ByteSlice};
use gix_glob::Pattern;
use gix_glob::pattern::Case;
use gix_glob::wildmatch;

/// The final component of an `include` entry: which names it is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    /// `.conf`
    Conf,
    /// `.config`
    Config,
    /// `.dotfile`
    Dotfile,
    /// `_empty_`: a name carrying no extension at all.
    Empty,
}

impl Token {
    /// The spelling an `include` block writes, and the one a diagnostic uses.
    pub fn name(self) -> &'static str {
        match self {
            Token::Conf => ".conf",
            Token::Config => ".config",
            Token::Dotfile => ".dotfile",
            Token::Empty => "_empty_",
        }
    }

    /// Read a token, or `None` for a word that is not one.
    pub fn parse(text: &str) -> Option<Token> {
        match text {
            ".conf" => Some(Token::Conf),
            ".config" => Some(Token::Config),
            ".dotfile" => Some(Token::Dotfile),
            "_empty_" => Some(Token::Empty),
            _ => None,
        }
    }

    /// The token a file's own name carries, or `None` for a name no token
    /// covers.
    ///
    /// The extension is `Path`'s, so a leading dot is part of the name rather
    /// than an extension: a file called `.conf` carries no extension and is
    /// `_empty_`, which is the same answer `native::kind` gives it.
    pub fn of(path: &Path) -> Option<Token> {
        path.file_name()?;
        match path.extension() {
            None => Some(Token::Empty),
            Some(extension) => match extension.to_str()? {
                "conf" => Some(Token::Conf),
                "config" => Some(Token::Config),
                "dotfile" => Some(Token::Dotfile),
                _ => None,
            },
        }
    }
}

/// The directory part of an `include` entry.
#[derive(Debug)]
enum Where {
    /// The entry was a bare token, so it is about every directory there is.
    Anywhere,
    /// The entry was written `/<token>`, so it is about the directory the
    /// config itself sits in and no directory below it.
    Root,
    /// A pattern. The entry is about the directories it matches, and about
    /// everything below one of them.
    ///
    /// One pattern, so one *component* unless it holds a `/`: a bare `ssh`
    /// matches a directory named exactly that at any depth, and `**ssh` is
    /// `*ssh`, which also takes `.ssh` and `openssh`.
    Under(Pattern),
}

/// One `include` entry.
#[derive(Debug)]
struct Rule {
    at: Where,
    token: Token,
    /// Written with a leading `!`, so it takes files back out again.
    negated: bool,
}

/// The `include` and `exclude` blocks of one config, ready to be asked about a
/// path.
///
/// `.dotfile` is included by default and the other three tokens are not:
/// `.dotfile` is dotfmt's own format and nothing else formats one, while the
/// files carrying no extension at all are this repository's `LICENSE`, its two
/// git hooks and the KDE `kdeglobals` — laying those out because somebody
/// pointed a formatter at the tree would be an unpleasant surprise.
#[derive(Debug)]
pub struct Selection {
    include: Vec<Rule>,
    exclude: Vec<Pattern>,
}

impl Default for Selection {
    fn default() -> Selection {
        Selection {
            include: vec![Rule {
                at: Where::Anywhere,
                token: Token::Dotfile,
                negated: false,
            }],
            exclude: Vec::new(),
        }
    }
}

impl Selection {
    /// Add one `include` entry, or say what is wrong with it.
    ///
    /// The built-in `.dotfile` entry is already there and this appends, so a
    /// config adds to the defaults and `!.dotfile` takes the default away —
    /// the same "later wins" rule that holds between two written entries.
    pub fn include(&mut self, entry: &str) -> Result<(), String> {
        let (negated, body) = match entry.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, entry),
        };
        if body.is_empty() {
            return Err(shape(entry, "there is nothing in it"));
        }
        let (before, last) = match body.rsplit_once('/') {
            Some((before, last)) => (Some(before), last),
            None => (None, body),
        };
        if last.is_empty() {
            return Err(shape(entry, "it ends in /, which names a directory"));
        }
        let Some(token) = Token::parse(last) else {
            return Err(shape(entry, &format!("{last} is not a token")));
        };
        let at = match before {
            None => Where::Anywhere,
            Some(before) => {
                spellable(before)?;
                placement(before)
                    .ok_or_else(|| shape(entry, &format!("{before} is not a directory pattern")))?
            }
        };
        self.include.push(Rule { at, token, negated });
        Ok(())
    }

    /// Add one `exclude` entry, or say what is wrong with it.
    ///
    /// The line is parsed exactly as it was written: `gix-glob` takes the
    /// leading `!`, the leading `/` and the trailing `/` off itself and
    /// records what each meant. Entries are asked in reverse below, so a later
    /// one wins over an earlier one, which is how git reads a `.gitignore`.
    ///
    /// A token in an `exclude` entry is refused rather than taken literally.
    /// `exclude { .conf }` reads as "no `.conf` files" and means "no file
    /// *named* `.conf`", which is the sort of thing that is only ever noticed
    /// by the diff it failed to prevent.
    pub fn exclude(&mut self, entry: &str) -> Result<(), String> {
        let body = entry.strip_prefix('!').unwrap_or(entry);
        let last = body.rsplit('/').next().unwrap_or(body);
        if let Some(token) = Token::parse(last) {
            return Err(format!(
                "{} is an include token; an exclude entry is a plain pattern",
                token.name()
            ));
        }
        spellable(entry)?;
        let pattern = Pattern::from_bytes(entry.as_bytes())
            .ok_or_else(|| format!("empty pattern: {entry}"))?;
        self.exclude.push(pattern);
        Ok(())
    }

    /// The token a file is picked up by, or `None` for one these settings
    /// leave alone. `relative` is the path below the directory the patterns
    /// were written in.
    pub fn owns(&self, relative: &Path) -> Option<Token> {
        let found = Token::of(relative)?;
        // A name that is not UTF-8 cannot be written as a pattern either, so
        // there is no entry that could have picked it up.
        let path = relative.to_str()?.as_bytes().as_bstr();
        let mut taken = false;
        for rule in &self.include {
            if rule.token == found && rule.covers(path) {
                taken = !rule.negated;
            }
        }
        (taken && !self.shut_out(path)).then_some(found)
    }

    /// Whether the `exclude` block turns a path away.
    ///
    /// Each directory on the way down is asked first, and the first one that
    /// is excluded takes the file with it — which is git's rule, and the
    /// reason a `!` cannot bring one file back out of an excluded directory.
    fn shut_out(&self, path: &BStr) -> bool {
        for directory in directories(path) {
            if self.verdict(directory, true) == Some(true) {
                return true;
            }
        }
        self.verdict(path, false) == Some(true)
    }

    /// What the last `exclude` pattern to match has to say: `Some(true)` to
    /// turn the path away, `Some(false)` for a `!` that keeps it, `None` when
    /// nothing matched at all.
    ///
    /// Asked in reverse and stopped at the first answer, which is git's "the
    /// last pattern to match decides" read from the other end.
    fn verdict(&self, path: &BStr, is_dir: bool) -> Option<bool> {
        self.exclude
            .iter()
            .rev()
            .find(|pattern| matches(pattern, path, is_dir))
            .map(|pattern| !pattern.is_negative())
    }
}

impl Rule {
    /// Whether this entry is about the directory the file sits in.
    fn covers(&self, path: &BStr) -> bool {
        match &self.at {
            Where::Anywhere => true,
            Where::Root => !path.contains(&b'/'),
            Where::Under(pattern) => {
                directories(path).any(|directory| matches(pattern, directory, true))
            }
        }
    }
}

/// Read the directory part of an `include` entry.
///
/// A trailing `/**` comes off first. `<dir>/**` and `<dir>` name the same set
/// of files here, because a directory already holds everything below it, and
/// taking it off is what makes `one/**/.conf` pick up `one/b.conf` the way
/// git's own `a/**/b` matches `a/b`.
fn placement(before: &str) -> Option<Where> {
    let mut before = before;
    let mut spanned = false;
    while let Some(rest) = before.strip_suffix("/**") {
        before = rest;
        spanned = true;
    }
    if before == "**" {
        return Some(Where::Anywhere);
    }
    if before.is_empty() {
        // `/**/<token>` spans everything below the config; `/<token>` is the
        // config's own directory and nothing below it.
        return Some(if spanned {
            Where::Anywhere
        } else {
            Where::Root
        });
    }
    // The `!` was taken off the whole entry already, so any `!` left in the
    // directory part is a character somebody meant literally.
    Pattern::from_bytes_without_negation(before.as_bytes()).map(Where::Under)
}

/// Every directory a path sits below, shallowest first, as the paths a
/// .gitignore pattern would be asked about on the way down.
fn directories(path: &BStr) -> impl Iterator<Item = &BStr> {
    path.find_iter("/").map(|at| path[..at].as_bstr())
}

/// One pattern against one path, in the mode a `.gitignore` is read in.
///
/// The basename position is computed rather than guessed at: gix asserts it
/// equals `rfind('/') + 1` in debug and quietly matches the wrong slice of the
/// path in release, so `None` here would be a bug that only shows up in a
/// build nobody runs the tests against.
fn matches(pattern: &Pattern, path: &BStr, is_dir: bool) -> bool {
    pattern.matches_repo_relative_path(
        path,
        path.rfind_byte(b'/').map(|at| at + 1),
        Some(is_dir),
        Case::Sensitive,
        wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
    )
}

/// Refuse the two spellings this grammar cannot carry.
///
/// An `=` never reaches here — `block.rs` reads a line holding one as
/// `key = value` and the config parser turns it away by its class — but a
/// pattern that gained one later would be *rewritten* by dotfmt laying its own
/// config out, so it is named here as well as there.
///
/// A trailing `\` in a `.gitignore` escapes a trailing space. `block.rs` has
/// already taken the trailing whitespace off the line, so there is nothing
/// left for it to escape and gix's wildmatch answers "no match" to everything
/// — a pattern that quietly does nothing at all.
fn spellable(text: &str) -> Result<(), String> {
    if text.contains('=') {
        return Err(format!(
            "a pattern cannot hold an =, which reads as key = value: {text}"
        ));
    }
    if text.ends_with('\\') {
        return Err(format!(
            "a pattern cannot end in \\, which would escape a trailing space \
             this file no longer has: {text}"
        ));
    }
    Ok(())
}

fn shape(entry: &str, why: &str) -> String {
    format!(
        "{entry} is not an include entry: {why}. \
         An entry ends in .conf, .config, .dotfile or _empty_"
    )
}
