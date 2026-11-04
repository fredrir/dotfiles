//! Engine dispatch: one arm per [`Engine`], no wildcard.

pub mod dash;
pub mod generic;
pub mod markup;
pub mod template;

use crate::lang::Dialect;
use crate::scan::{Bail, Out};
use crate::syntax::{Engine, Syntax};
use crate::{css, curly, hash, js};

/// Lex `source` with the engine its descriptor names.
///
/// Only [`Engine::Generic`] and [`Engine::Elixir`] are descriptor-complete this
/// wave; the irregular engines still match on `Dialect` and cannot be reached
/// from a `Syntax` alone (see [`scan_native`]).
pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    match syntax.engine {
        Engine::Generic => generic::scan(source, syntax),
        Engine::Curly => curly::scan(source, syntax),
        Engine::Dash => dash::scan(source, syntax),
        // Elixir is not a separate lexer: the descriptor is run by the generic
        // engine, and the arm exists so HEEx/EEX `<%= … %>` bodies can be lexed
        // without inventing a `Region.syntax` resolver.
        Engine::Elixir => generic::scan(source, Dialect::Elixir.syntax()),
        // TODO(wave 2): `engine/{indented,shell,ruby}.rs` plus the js/css/hash
        // ports read the descriptor, and this arm goes with `scan_native`. Until
        // then a `Syntax` cannot be mapped back to a `Dialect` without guessing
        // (`Mjs`/`JavaScript`, `Scss`/`Sass`, `Tsx`/`TypeScript` share
        // descriptors but not behaviour).
        Engine::Script | Engine::Style | Engine::Indented | Engine::Shell | Engine::Ruby => {
            Err(Bail("engine dispatch needs the dialect"))
        }
        // TODO(wave 2): `engine/markup.rs` and `engine/template.rs` replace this;
        // a region that opens the verbatim engine can only be closed by the
        // region stack that opened it.
        Engine::Markup => markup::scan(source, syntax),
        Engine::Template => template::scan(source, syntax),
        Engine::Verbatim => Err(Bail("verbatim region outside markup")),
    }
}

// TODO(wave 2): delete `scan_native` once js/css/hash read the descriptor; every
// arm below is a remaining `Dialect` match that the descriptor is supposed to replace.
//
// `scan::scan` calls this rather than `scan` because the irregular engines need
// the `Dialect` they still match on, and recovering it from a `Syntax` is lossy.
pub(crate) fn scan_native(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    let syntax = dialect.syntax();
    match syntax.engine {
        Engine::Script => js::scan(source, dialect),
        Engine::Style => css::scan(source, dialect),
        // The legacy hash scanners only know the five dialects that predate the
        // descriptor. Every other dialect naming one of these engines answers
        // with a skip rather than an empty `Out`: an empty `Out` means "this
        // file has no comments", which is indistinguishable from "this scanner
        // does not know this language" and is how the original fail-silent bug
        // hid. A skip is visible in the report and in `--check`.
        Engine::Indented | Engine::Shell | Engine::Ruby if hash::handles(dialect) => {
            hash::scan(source, dialect)
        }
        Engine::Indented | Engine::Shell | Engine::Ruby => {
            Err(Bail("no scanner for this dialect yet"))
        }
        Engine::Elixir => generic::scan(source, syntax),
        Engine::Verbatim => Err(Bail("verbatim region outside markup")),
        Engine::Markup => markup::scan(source, syntax),
        Engine::Template => template::scan(source, syntax),
        // The generic engine replaces the legacy family scanners one family at a
        // time; until then `curly`/`dash` keep today's byte-for-byte behaviour.
        // Every Generic descriptor is served by the table-driven lexer. The
        // legacy family scanners are only reached through their own Engine
        // variants above, so a descriptor can never silently fall back to a
        // scanner that does not know its language.
        Engine::Generic => generic::scan(source, syntax),
        Engine::Curly => curly::scan(source, syntax),
        Engine::Dash => dash::scan(source, syntax),
    }
}
