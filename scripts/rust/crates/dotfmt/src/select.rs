
use std::path::Path;

use bstr::{BStr, ByteSlice};
use gix_glob::Pattern;
use gix_glob::pattern::Case;
use gix_glob::wildmatch;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    Conf,
    Config,
    Dotfile,
    Empty,
}

impl Token {
    pub fn name(self) -> &'static str {
        match self {
            Token::Conf => ".conf",
            Token::Config => ".config",
            Token::Dotfile => ".dotfile",
            Token::Empty => "_empty_",
        }
    }

    pub fn parse(text: &str) -> Option<Token> {
        match text {
            ".conf" => Some(Token::Conf),
            ".config" => Some(Token::Config),
            ".dotfile" => Some(Token::Dotfile),
            "_empty_" => Some(Token::Empty),
            _ => None,
        }
    }

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

#[derive(Debug)]
enum Where {
    Anywhere,
    Root,
    Under(Pattern),
}

#[derive(Debug)]
struct Rule {
    at: Where,
    token: Token,
    negated: bool,
}

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

    fn shut_out(&self, path: &BStr) -> bool {
        for directory in directories(path) {
            if self.verdict(directory, true) == Some(true) {
                return true;
            }
        }
        self.verdict(path, false) == Some(true)
    }

    fn verdict(&self, path: &BStr, is_dir: bool) -> Option<bool> {
        self.exclude
            .iter()
            .rev()
            .find(|pattern| matches(pattern, path, is_dir))
            .map(|pattern| !pattern.is_negative())
    }
}

impl Rule {
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

fn placement(before: &str) -> Option<Where> {
    let mut before = before;
    let mut spanned = false;
    while let Some(rest) = before.strip_suffix("<token>` spans everything below the config; `/<token>` is the
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

fn directories(path: &BStr) -> impl Iterator<Item = &BStr> {
    path.find_iter("/").map(|at| path[..at].as_bstr())
}

fn matches(pattern: &Pattern, path: &BStr, is_dir: bool) -> bool {
    pattern.matches_repo_relative_path(
        path,
        path.rfind_byte(b'/').map(|at| at + 1),
        Some(is_dir),
        Case::Sensitive,
        wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
    )
}

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
