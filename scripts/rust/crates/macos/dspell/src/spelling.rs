#![allow(unsafe_code)]

use std::ops::Range;
use std::ptr;

use objc2::rc::Retained;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSInteger, NSNotFound, NSRange, NSString};

pub struct Spelling {
    checker: Retained<NSSpellChecker>,
    text: Retained<NSString>,
}

impl Spelling {
    pub fn new(text: &str) -> Spelling {
        Spelling {
            checker: NSSpellChecker::sharedSpellChecker(),
            text: NSString::from_str(text),
        }
    }

    pub fn supports(&self, language: &str) -> bool {
        self.checker
            .availableLanguages()
            .iter()
            .any(|available| available.to_string() == language)
    }

    pub fn utf16_misspellings(&self, language: &str) -> Vec<Range<usize>> {
        let language = NSString::from_str(language);
        let mut words = Vec::new();
        let mut offset = 0;
        loop {
            // SAFETY: AppKit skips the word count when its out-pointer is null.
            let word = unsafe {
                self.checker
                    .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                        &self.text,
                        offset,
                        Some(&language),
                        false,
                        0,
                        ptr::null_mut(),
                    )
            };
            if word.location == NSNotFound as usize || word.length == 0 {
                return words;
            }
            offset = word.end() as NSInteger;
            words.push(word.location..word.end());
        }
    }

    pub fn guesses(&self, utf16_word: Range<usize>, language: &str) -> Vec<String> {
        self.checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                NSRange::from(utf16_word),
                &self.text,
                Some(&NSString::from_str(language)),
                0,
            )
            .map(|guesses| guesses.iter().map(|guess| guess.to_string()).collect())
            .unwrap_or_default()
    }
}
