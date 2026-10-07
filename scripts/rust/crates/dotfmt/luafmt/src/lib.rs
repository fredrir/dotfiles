#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;

use config::Config;
use full_moon::tokenizer::{Lexer, LexerResult, TokenType};
use stylua_lib::OutputVerification;

pub fn format(input: &str, config: &Config) -> Result<String, String> {
    format_with_dialect(input, config, config.dialect)
}

pub fn format_with_dialect(
    input: &str,
    config: &Config,
    dialect: dialect::Dialect,
) -> Result<String, String> {
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
        .map_err(|error| error.to_string())?;
    if !config.final_newline {
        output.truncate(output.trim_end_matches(['\r', '\n']).len());
    }
    if config.verify {
        // StyLua's AST comparison panics on valid hex floats and rejects its
        // own require sorting. Validate the exact emitted text instead.
        full_moon::parse_fallible(&output, style.syntax.into())
            .into_result()
            .map_err(|errors| stylua_lib::Error::VerificationAstError(errors).to_string())?;
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

fn reject_interpolated_strings(input: &str) -> Result<(), String> {
    // full_moon 2.2 panics on backtick tokens outside Luau. Lex only inputs
    // containing backticks with all lexical extensions first, so strings and comments
    // containing literal backticks are still accepted without a custom scanner.
    let tokens = match Lexer::new(input, full_moon::LuaVersion::new()).collect() {
        LexerResult::Ok(tokens) => tokens,
        LexerResult::Fatal(errors) | LexerResult::Recovered(_, errors) => {
            return Err(format!(
                "error parsing: {}",
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    };
    if let Some(token) = tokens
        .iter()
        .find(|token| matches!(token.token_type(), TokenType::InterpolatedString { .. }))
    {
        let at = token.start_position();
        return Err(format!(
            "error parsing: unexpected backtick at {}:{}; use --dialect luau for interpolated strings",
            at.line(),
            at.character()
        ));
    }
    Ok(())
}
