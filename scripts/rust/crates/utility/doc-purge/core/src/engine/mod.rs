//! Engine dispatch: one arm per [`Engine`], no wildcard.

pub mod dash;
pub mod generic;
pub mod markup;
pub mod perl;
pub mod template;

use crate::scan::{Bail, Out};
use crate::syntax::{Engine, Syntax};
use crate::{css, curly, hash, js};

/// Lex `source` with the engine its descriptor names.
pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    match syntax.engine {
        Engine::Generic => generic::scan(source, syntax),
        Engine::Curly => curly::scan(source, syntax),
        Engine::Dash => dash::scan(source, syntax),
        Engine::Perl => perl::scan(source, syntax),
        // Elixir is not a separate lexer: the descriptor is run by the generic
        // engine, and the arm exists so HEEx/EEX `<%= … %>` bodies can be lexed
        // without inventing a `Region.syntax` resolver.
        // Elixir inside HEEx/EEX: the descriptor resolves to the Elixir table, which
    // the generic lexer runs. Looked up by name so this module never names a
    // `Dialect`; the descriptor is the only thing that knows a language.
    Engine::Elixir => generic::scan(source, named("elixir")),
        Engine::Script => js::scan(source, syntax),
        Engine::Style => css::scan(source, syntax),
        Engine::Indented | Engine::Shell | Engine::Ruby => hash::scan(source, syntax),
        // TODO(wave 2): `engine/markup.rs` and `engine/template.rs` replace this;
        // a region that opens the verbatim engine can only be closed by the
        // region stack that opened it.
        Engine::Markup => markup::scan(source, syntax),
        Engine::Template => template::scan(source, syntax),
        Engine::Verbatim => Err(Bail("verbatim region outside markup")),
    }
}

/// The descriptor with this name.
///
/// Panics only if the registry loses a name the tables promise, which the
/// registry test forbids; every other path in this module is `Bail`.
fn named(name: &'static str) -> &'static Syntax {
    crate::lang::LANGUAGES
        .iter()
        .find(|language| language.name == name)
        .map(|language| language.dialect.syntax())
        .expect("named descriptor is in LANGUAGES")
}
