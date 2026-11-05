
pub mod dash;
pub mod generic;
pub mod markup;
pub mod perl;
pub mod template;

use crate::scan::{Bail, Out};
use crate::syntax::{Engine, Syntax};
use crate::{css, curly, hash, js};

pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    match syntax.engine {
        Engine::Generic => generic::scan(source, syntax),
        Engine::Curly => curly::scan(source, syntax),
        Engine::Dash => dash::scan(source, syntax),
        Engine::Perl => perl::scan(source, syntax),
        Engine::Elixir => generic::scan(source, named("elixir")),
        Engine::Script => js::scan(source, syntax),
        Engine::Style => css::scan(source, syntax),
        Engine::Indented | Engine::Shell | Engine::Ruby => hash::scan(source, syntax),
        Engine::Markup => markup::scan(source, syntax),
        Engine::Template => template::scan(source, syntax),
        Engine::Verbatim => Err(Bail("verbatim region outside markup")),
    }
}

fn named(name: &'static str) -> &'static Syntax {
    crate::lang::LANGUAGES
        .iter()
        .find(|language| language.name == name)
        .map(|language| language.dialect.syntax())
        .expect("named descriptor is in LANGUAGES")
}
