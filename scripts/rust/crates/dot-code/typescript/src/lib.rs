#![forbid(unsafe_code)]

mod convert;
mod extract;
mod layout;
mod names;
mod path;
pub mod settings;

use dot_code_core::{Action, Feature, Request, SettingsError, Value};
use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

pub use settings::Settings;

pub const EXTRACT_INTERFACE: &str = extract::INTERFACE;
pub const EXTRACT_TYPE: &str = extract::TYPE;
pub const CONVERT_TO_INTERFACE: &str = convert::INTERFACE;
pub const CONVERT_TO_TYPE: &str = convert::TYPE;

const KINDS: &[&str] = &[
    EXTRACT_INTERFACE,
    EXTRACT_TYPE,
    CONVERT_TO_INTERFACE,
    CONVERT_TO_TYPE,
];

#[derive(Default)]
pub struct TypeScript {
    allocator: Allocator,
    settings: Settings,
}

impl Feature for TypeScript {
    fn section(&self) -> &'static str {
        "typescript"
    }

    fn languages(&self) -> &'static [&'static str] {
        &["typescript", "typescriptreact"]
    }

    fn kinds(&self) -> &'static [&'static str] {
        KINDS
    }

    fn configure(&mut self, settings: Value) -> Result<(), SettingsError> {
        self.settings = serde_json::from_value(settings)?;
        Ok(())
    }

    fn code_actions(&mut self, request: &Request) -> Vec<Action> {
        if !KINDS.iter().any(|kind| request.wants(kind)) {
            return Vec::new();
        }
        self.allocator.reset();
        let source_type = if request.language == "typescriptreact" {
            SourceType::tsx()
        } else {
            SourceType::ts()
        };
        let parsed = Parser::new(&self.allocator, request.source, source_type)
            .with_options(ParseOptions {
                preserve_parens: false,
                ..ParseOptions::default()
            })
            .parse();
        if parsed.fatal_error {
            return Vec::new();
        }
        let program = &parsed.program;
        let path = path::enclosing(program, &request.selection);
        let mut actions = extract::actions(program, &path, request, &self.settings.extract);
        actions.extend(convert::actions(program, &path, request));
        actions
    }
}
