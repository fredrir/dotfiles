use ignore::gitignore::GitignoreBuilder;

use crate::syntax::{QuoteMode, scan};

use super::*;

pub(super) fn parse(source: &str, path: &Path, root: &Path) -> Result<Layer, Diagnostic> {
    let mut parser = Parser {
        source,
        offset: 0,
        line: 1,
        path,
        root,
    };
    let mut layer = Layer::default();
    let mut global_seen = false;
    while parser.skip() {
        let line = parser.line;
        let name = parser.head()?;
        match name.as_str() {
            "" => {
                if global_seen {
                    return Err(parser.error(line, "duplicate global block"));
                }
                global_seen = true;
                let local = parser.settings(true)?;
                layer.global = local.settings;
                if local.includes.is_some() {
                    if layer.includes.is_some() {
                        return Err(parser.error(line, "duplicate included_files block"));
                    }
                    layer.includes = local.includes;
                }
                if local.excludes.is_some() {
                    if layer.excludes.is_some() {
                        return Err(parser.error(line, "duplicate excluded_files block"));
                    }
                    layer.excludes = local.excludes;
                }
            }
            "included_files" | "include" => {
                if layer.includes.is_some() {
                    return Err(parser.error(line, "duplicate included_files block"));
                }
                layer.includes = Some(parser.patterns()?);
            }
            "excluded_files" | "exclude" => {
                if layer.excludes.is_some() {
                    return Err(parser.error(line, "duplicate excluded_files block"));
                }
                layer.excludes = Some(parser.patterns()?);
            }
            _ => {
                let language =
                    Language::parse(&name).map_err(|message| parser.error(line, message))?;
                if layer.languages.contains_key(&language) {
                    return Err(parser.error(line, format!("duplicate {} block", language.name())));
                }
                layer.languages.insert(language, parser.settings(false)?);
            }
        }
    }
    Ok(layer)
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    line: usize,
    path: &'a Path,
    root: &'a Path,
}

impl Parser<'_> {
    fn error(&self, line: usize, message: impl Display) -> Diagnostic {
        Diagnostic::config(self.path, line, message.to_string())
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
        }
        Some(ch)
    }

    fn skip(&mut self) -> bool {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() != Some('#') {
                return self.peek().is_some();
            }
            while self.peek().is_some_and(|ch| ch != '\n') {
                self.bump();
            }
        }
    }

    fn head(&mut self) -> Result<String, Diagnostic> {
        let start = self.offset;
        let line = self.line;
        while let Some(ch) = self.peek() {
            if ch == '{' {
                let name = self.source[start..self.offset].trim().replace('-', "_");
                self.bump();
                return Ok(name);
            }
            if matches!(ch, '}' | '\n' | '=' | '#') {
                break;
            }
            self.bump();
        }
        Err(self.error(line, "expected a block followed by '{'"))
    }

    fn settings(&mut self, global: bool) -> Result<Local, Diagnostic> {
        let mut local = Local {
            enabled: true,
            settings: Settings::new(),
            includes: None,
            excludes: None,
        };
        let mut enabled_seen = false;
        loop {
            if !self.skip() {
                return Err(self.error(self.line, "unterminated configuration block"));
            }
            if self.peek() == Some('}') {
                self.bump();
                return Ok(local);
            }
            let start = self.offset;
            let line = self.line;
            while self
                .peek()
                .is_some_and(|ch| !matches!(ch, '=' | '{' | '}' | '\n' | '#'))
            {
                self.bump();
            }
            let name = self.source[start..self.offset].trim().replace('-', "_");
            if self.peek() == Some('{') {
                self.bump();
                let target = match name.as_str() {
                    "included_files" | "include" => &mut local.includes,
                    "excluded_files" | "exclude" => &mut local.excludes,
                    _ => return Err(self.error(line, format!("unknown nested block '{name}'"))),
                };
                if target.is_some() {
                    return Err(self.error(line, format!("duplicate {name} block")));
                }
                *target = Some(self.patterns()?);
                continue;
            }
            if self.peek() != Some('=') || name.is_empty() || name.chars().any(char::is_whitespace)
            {
                return Err(self.error(line, "expected a setting in the form key = value"));
            }
            self.bump();
            let value = self.value()?;
            if name == "enabled" && !global {
                if enabled_seen {
                    return Err(self.error(line, "duplicate setting 'enabled'"));
                }
                enabled_seen = true;
                local.enabled = match value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(self.error(line, "enabled must be true or false")),
                };
                continue;
            }
            if global {
                match name.as_str() {
                    "width" | "indent" => {
                        if value
                            .parse::<usize>()
                            .ok()
                            .filter(|number| *number > 0)
                            .is_none()
                        {
                            return Err(
                                self.error(line, format!("{name} must be a positive integer"))
                            );
                        }
                    }
                    "final_newline" => {
                        if value != "true" && value != "false" {
                            return Err(self.error(line, "final_newline must be true or false"));
                        }
                    }
                    "quote_style" => {
                        if !matches!(
                            value.as_str(),
                            "double"
                                | "single"
                                | "auto"
                                | "auto-prefer-double"
                                | "auto-prefer-single"
                        ) {
                            return Err(self.error(line, "quote_style must be double, single, auto, auto-prefer-double, or auto-prefer-single"));
                        }
                    }
                    _ => return Err(self.error(line, format!("unknown global setting '{name}'"))),
                }
            }
            let setting = Setting {
                value,
                source: self.path.to_path_buf(),
                line,
                global,
            };
            if local.settings.insert(name.clone(), setting).is_some() {
                return Err(self.error(line, format!("duplicate setting '{name}'")));
            }
        }
    }

    fn value(&mut self) -> Result<String, Diagnostic> {
        while self.peek().is_some_and(|ch| ch == ' ' || ch == '\t') {
            self.bump();
        }
        let start = self.offset;
        let line = self.line;
        if matches!(self.peek(), Some('"' | '\'')) {
            let quote = self.bump().unwrap_or('"');
            let mut value = String::new();
            loop {
                match self.bump() {
                    Some(ch) if ch == quote => break,
                    Some('\\') => match self.bump() {
                        Some(ch) if ch == quote || ch == '\\' => value.push(ch),
                        Some(ch) => {
                            value.push('\\');
                            value.push(ch);
                        }
                        None => return Err(self.error(line, "unterminated quoted value")),
                    },
                    Some('\n') | None => return Err(self.error(line, "unterminated quoted value")),
                    Some(ch) => value.push(ch),
                }
            }
            while self
                .peek()
                .is_some_and(|ch| ch == ' ' || ch == '\t' || ch == '\r')
            {
                self.bump();
            }
            if self
                .peek()
                .is_some_and(|ch| !matches!(ch, '\n' | '#' | '}'))
            {
                return Err(self.error(line, "unexpected text after quoted value"));
            }
            return Ok(value);
        }
        while self
            .peek()
            .is_some_and(|ch| !matches!(ch, '\n' | '}' | '#'))
        {
            self.bump();
        }
        let value = self.source[start..self.offset].trim();
        if value.is_empty() {
            return Err(self.error(line, "missing setting value"));
        }
        Ok(value.to_string())
    }

    fn patterns(&mut self) -> Result<Arc<PatternLayer>, Diagnostic> {
        let mut builder = GitignoreBuilder::new(self.root);
        loop {
            if !self.skip() {
                return Err(self.error(self.line, "unterminated file-pattern block"));
            }
            if self.peek() == Some('}') {
                self.bump();
                let matcher = builder
                    .build()
                    .map_err(|error| self.error(self.line, error))?;
                return Ok(Arc::new(PatternLayer {
                    root: self.root.to_path_buf(),
                    matcher,
                }));
            }
            let start = self.offset;
            let line = self.line;
            let mut separator = false;
            for token in scan(&self.source[start..], QuoteMode::Start) {
                let ch = token.character;
                if ch == '\n' || (ch == '}' && token.is_structural() && separator) {
                    break;
                }
                separator = ch.is_whitespace() && token.is_structural();
                self.bump();
            }
            let pattern = &self.source[start..self.offset];
            // globset also implements shell brace alternatives; gitignore
            // treats braces literally, including an unmatched opening brace.
            let mut literal = String::with_capacity(pattern.len());
            let mut escaped = false;
            let mut class = false;
            for ch in pattern.chars() {
                if !escaped {
                    if ch == '[' {
                        class = true;
                    }
                    if ch == ']' {
                        class = false;
                    }
                    if !class && matches!(ch, '{' | '}') {
                        literal.push('\\');
                    }
                }
                literal.push(ch);
                escaped = ch == '\\' && !escaped;
            }
            builder
                .add_line(Some(self.path.to_path_buf()), &literal)
                .map_err(|error| self.error(line, error))?;
        }
    }
}
