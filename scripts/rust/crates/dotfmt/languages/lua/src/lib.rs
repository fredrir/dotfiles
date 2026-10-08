#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;

use config::Config;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use full_moon::tokenizer::{Lexer, LexerResult, TokenType};
use stylua_lib::OutputVerification;

pub fn format(input: &str, config: &Config) -> Result<String, Diagnostic> {
    format_with_dialect(input, config, config.dialect)
}

pub fn format_with_dialect(
    input: &str,
    config: &Config,
    dialect: dialect::Dialect,
) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    let mut style = config.style;
    style.syntax = dialect.syntax();
    if !matches!(dialect, dialect::Dialect::Luau | dialect::Dialect::All) && input.contains('`') {
        reject_interpolated_strings(input)?;
    }
    let mut output = stylua_lib::format_code(input, style, None, OutputVerification::None)
        .map_err(diagnostic)?;
    if !config.final_newline {
        output.truncate(output.trim_end_matches(['\r', '\n']).len());
    }
    if config.verify {
        // StyLua's AST comparison panics on valid hex floats and rejects its
        // own require sorting. Validate the exact emitted text instead.
        full_moon::parse_fallible(&output, style.syntax.into())
            .into_result()
            .map_err(|errors| diagnostic(stylua_lib::Error::VerificationAstError(errors)))?;
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

fn reject_interpolated_strings(input: &str) -> Result<(), Diagnostic> {
    // full_moon 2.2 panics on backtick tokens outside Luau. Lex only inputs
    // containing backticks with all lexical extensions first, so strings and comments
    // containing literal backticks are still accepted without a custom scanner.
    let tokens = match Lexer::new(input, full_moon::LuaVersion::new()).collect() {
        LexerResult::Ok(tokens) => tokens,
        LexerResult::Fatal(errors) | LexerResult::Recovered(_, errors) => {
            let mut diagnostic = Diagnostic::new(
                DiagnosticKind::Syntax,
                format!(
                    "error parsing: {}",
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            );
            if let Some(error) = errors.first() {
                let at = error.range().0;
                diagnostic = diagnostic.with_location(at.line(), Some(at.character()));
            }
            return Err(diagnostic);
        }
    };
    if let Some(token) = tokens
        .iter()
        .find(|token| matches!(token.token_type(), TokenType::InterpolatedString { .. }))
    {
        let at = token.start_position();
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            "error parsing: unexpected backtick; use --dialect luau for interpolated strings",
        )
        .with_location(at.line(), Some(at.character())));
    }
    Ok(())
}

fn diagnostic(error: stylua_lib::Error) -> Diagnostic {
    let (kind, position) = match &error {
        stylua_lib::Error::ParseError(errors) => (
            DiagnosticKind::Syntax,
            errors.first().map(|error| error.range().0),
        ),
        stylua_lib::Error::VerificationAstError(errors) => (
            DiagnosticKind::Internal,
            errors.first().map(|error| error.range().0),
        ),
        _ => (DiagnosticKind::Internal, None),
    };
    let diagnostic = Diagnostic::new(kind, error.to_string());
    match position {
        Some(at) => diagnostic.with_location(at.line(), Some(at.character())),
        None => diagnostic,
    }
}
