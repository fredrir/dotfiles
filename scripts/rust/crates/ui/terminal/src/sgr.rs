use std::io::{self, Write};
use ui_theme::{Color, ColorDepth};

const MAX_SEQUENCE: usize = 128;

pub struct SgrWriter<W> {
    inner: W,
    strict: bool,
    pending: Vec<u8>,
}

impl<W: Write> SgrWriter<W> {
    pub fn new(inner: W) -> Self {
        Self::with_depth(inner, ColorDepth::detect())
    }
    pub fn with_depth(inner: W, depth: ColorDepth) -> Self {
        Self {
            inner,
            strict: depth == ColorDepth::Ansi16,
            pending: Vec::new(),
        }
    }
    pub fn get_ref(&self) -> &W {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    fn finish_sequence(&mut self) -> io::Result<()> {
        if let Some(converted) = rewrite(&self.pending) {
            self.inner.write_all(converted.as_bytes())?;
        } else {
            self.inner.write_all(&self.pending)?;
        }
        self.pending.clear();
        Ok(())
    }
}

impl<W: Write> Write for SgrWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.strict {
            return self.inner.write(bytes);
        }
        let mut position = 0;
        while position < bytes.len() {
            if self.pending.is_empty() {
                let plain = bytes[position..]
                    .iter()
                    .position(|byte| *byte == 0x1b)
                    .unwrap_or(bytes.len() - position);
                self.inner.write_all(&bytes[position..position + plain])?;
                position += plain;
                if position == bytes.len() {
                    break;
                }
            }
            let byte = bytes[position];
            self.pending.push(byte);
            position += 1;
            let length = self.pending.len();
            if length == MAX_SEQUENCE
                || length == 2 && byte != b'['
                || length > 2 && (0x40..=0x7e).contains(&byte)
            {
                self.finish_sequence()?;
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            self.inner.write_all(&self.pending)?;
            self.pending.clear();
        }
        self.inner.flush()
    }
}

fn rewrite(sequence: &[u8]) -> Option<String> {
    let body = std::str::from_utf8(sequence)
        .ok()?
        .strip_prefix("\x1b[")?
        .strip_suffix('m')?;
    let parameters: Vec<_> = body.split(';').collect();
    let mut rewritten = Vec::with_capacity(parameters.len());
    let mut index = 0;
    let mut changed = false;
    while index < parameters.len() {
        let background = parameters[index] == "48";
        if matches!(parameters[index], "38" | "48") {
            let mode = parameters.get(index + 1)?;
            let (color, consumed) = match *mode {
                "5" => (Color::Ansi(parameters.get(index + 2)?.parse().ok()?), 3),
                "2" => (
                    Color::Rgb(
                        parameters.get(index + 2)?.parse().ok()?,
                        parameters.get(index + 3)?.parse().ok()?,
                        parameters.get(index + 4)?.parse().ok()?,
                    ),
                    5,
                ),
                _ => return None,
            };
            let rgb = color.rgb()?;
            let Color::Ansi(color) =
                Color::Rgb(rgb[0], rgb[1], rgb[2]).at_depth(ColorDepth::Ansi16)
            else {
                return None;
            };
            let code = match (background, color < 8) {
                (false, true) => 30 + color,
                (true, true) => 40 + color,
                (false, false) => 90 + color - 8,
                (true, false) => 100 + color - 8,
            };
            rewritten.push(code.to_string());
            index += consumed;
            changed = true;
        } else {
            rewritten.push(parameters[index].to_owned());
            index += 1;
        }
    }
    changed.then(|| format!("\x1b[{}m", rewritten.join(";")))
}

#[cfg(test)]
#[path = "../tests/unit/sgr_tests.rs"]
mod tests;
