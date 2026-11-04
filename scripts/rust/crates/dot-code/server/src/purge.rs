use std::path::PathBuf;

use doc_purge_core::lang;
use doc_purge_core::purge::{self, Outcome};
use lsp_types::TextEdit;
use serde::{Deserialize, Serialize};

use crate::documents::Document;

#[derive(Debug, Deserialize)]
pub struct Params {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Purged {
    pub edits: Vec<TextEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<&'static str>,
}

impl Purged {
    fn skipped(reason: &'static str) -> Self {
        Self {
            edits: Vec::new(),
            skipped: Some(reason),
        }
    }
}

pub fn purge(params: Params) -> Purged {
    let Some(language) = lang::for_path(&params.path) else {
        return Purged::skipped("not a file type doc-purge reads");
    };
    match purge::purge(params.text.as_bytes(), language.dialect) {
        Outcome::Skipped(reason) => Purged::skipped(reason),
        Outcome::Untouched(_) => Purged::default(),
        Outcome::Changed(edited, _) => {
            let document = Document::new(params.text, String::new(), 0);
            Purged {
                edits: edited
                    .changes
                    .into_iter()
                    .map(|change| TextEdit {
                        range: document.lsp_range(&change.range),
                        new_text: change.text,
                    })
                    .collect(),
                skipped: None,
            }
        }
    }
}
