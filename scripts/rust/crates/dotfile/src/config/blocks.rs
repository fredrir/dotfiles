use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub block: String,
    pub number: usize,
    pub text: String,
    pub opens: bool,
}

impl Entry {
    pub fn split(&self) -> (&str, &str) {
        self.text
            .split_once('=')
            .map_or((self.text.trim(), ""), |(key, value)| {
                (key.trim(), value.trim())
            })
    }
}

#[derive(Clone, Copy)]
pub enum Comments {
    Inline,
    Lines,
    None,
}

pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    parse_with_comments(text, Comments::Inline)
}

pub fn parse_with_comments(text: &str, comments: Comments) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut block: Option<&str> = None;
    for (offset, raw) in text.lines().enumerate() {
        let number = offset + 1;
        let line = match comments {
            Comments::Inline => raw.split('#').next().unwrap_or_default().trim(),
            Comments::Lines if raw.trim_start().starts_with('#') => "",
            _ => raw.trim(),
        };
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            if block.take().is_none() {
                return Err(format!("line {number}: unexpected }}"));
            }
        } else if let Some(name) = line.strip_suffix('{') {
            if block.is_some() {
                return Err(format!("line {number}: nested block"));
            }
            let name = name.trim();
            if name.is_empty() {
                return Err(format!("line {number}: block name missing"));
            }
            block = Some(name);
            entries.push(Entry {
                block: name.to_string(),
                number,
                text: String::new(),
                opens: true,
            });
        } else {
            let name = block.ok_or_else(|| format!("line {number}: entry outside a block"))?;
            entries.push(Entry {
                block: name.to_string(),
                number,
                text: line.to_string(),
                opens: false,
            });
        }
    }
    if let Some(block) = block {
        return Err(format!("missing }} for {block}"));
    }
    Ok(entries)
}

pub fn read(path: &Path) -> Result<Vec<Entry>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}
