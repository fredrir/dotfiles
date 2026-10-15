use std::iter;
use std::ops::Range;

use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

use crate::spelling::Spelling;

#[derive(Serialize)]
pub struct Typo {
    word: String,
    length: usize,
    offset: isize,
    guesses: Vec<String>,
}

pub fn find(before: &str, after: &str, languages: &[String]) -> Result<Typo, String> {
    let text = format!("{before}{after}");
    let cursor = before.encode_utf16().count();
    let spelling = Spelling::new(&text);
    if let Some(unknown) = languages
        .iter()
        .find(|language| !spelling.supports(language))
    {
        return Err(format!("unknown language: {unknown}"));
    }
    let flagged: Vec<_> = languages
        .iter()
        .map(|language| spelling.utf16_misspellings(language))
        .collect();
    // The language a text is written in flags the fewest of its words.
    let primary = (0..languages.len())
        .min_by_key(|&index| flagged[index].len())
        .ok_or("no language")?;
    let word = flagged[primary]
        .iter()
        .rev()
        .filter(|word| word.start <= cursor)
        .find(|word| {
            flagged
                .iter()
                .all(|words| words.iter().any(|other| overlaps(word, other)))
        })
        .ok_or("no typo found")?;
    let guesses = iter::once(primary)
        .chain((0..languages.len()).filter(|&index| index != primary))
        .map(|index| spelling.guesses(word.clone(), &languages[index]))
        .find(|guesses| !guesses.is_empty())
        .unwrap_or_default();

    let start = byte_offset(&text, word.start);
    let end = byte_offset(&text, word.end);
    let cursor = before.len();
    let offset = if end >= cursor {
        graphemes(&text[cursor..end]) as isize
    } else {
        -(graphemes(&text[end..cursor]) as isize)
    };
    Ok(Typo {
        word: text[start..end].to_string(),
        length: graphemes(&text[start..end]),
        offset,
        guesses,
    })
}

fn graphemes(text: &str) -> usize {
    text.graphemes(true).count()
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn byte_offset(text: &str, utf16_offset: usize) -> usize {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units >= utf16_offset {
            return index;
        }
        units += character.len_utf16();
    }
    text.len()
}
