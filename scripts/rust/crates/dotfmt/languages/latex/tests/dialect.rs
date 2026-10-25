#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_latex::dialect::Dialect;

#[test]
fn dialects_parse_canonical_names_and_variant_aliases() {
    for (value, dialect) in [
        ("auto", Dialect::Auto),
        ("latex", Dialect::Latex),
        ("pdflatex", Dialect::Latex),
        ("xelatex", Dialect::Latex),
        ("lualatex", Dialect::Latex),
        ("expl3", Dialect::Latex),
        ("beamer", Dialect::Latex),
        ("tikz", Dialect::Latex),
        ("plain", Dialect::Plain),
        ("tex", Dialect::Plain),
        ("optex", Dialect::Plain),
        ("context", Dialect::Context),
        ("ctx", Dialect::Context),
        ("mkiv", Dialect::Context),
        ("lmtx", Dialect::Context),
    ] {
        assert_eq!(Dialect::parse(value, false), Ok(dialect), "{value}");
    }
    assert_eq!(Dialect::parse("PDFLATEX", true), Ok(Dialect::Latex));
    assert!(Dialect::parse("unknown", true).is_err());
}

#[test]
fn extensions_detect_context_and_default_to_latex() {
    assert_eq!(
        Dialect::for_path(Path::new("paper.tex")),
        Some(Dialect::Latex)
    );
    assert_eq!(
        Dialect::for_path(Path::new("paper.STY")),
        Some(Dialect::Latex)
    );
    assert_eq!(
        Dialect::for_path(Path::new("context.ctx")),
        Some(Dialect::Context)
    );
    assert_eq!(
        Dialect::for_path(Path::new("context.mkiv")),
        Some(Dialect::Context)
    );
    assert_eq!(
        Dialect::for_path(Path::new("README.md")),
        Some(Dialect::Latex)
    );
    assert_eq!(
        Dialect::resolve(Dialect::Auto, Dialect::Auto, Path::new("a.ctx")),
        Dialect::Context
    );
    assert_eq!(
        Dialect::resolve(Dialect::Plain, Dialect::Latex, Path::new("a.ctx")),
        Dialect::Plain
    );
    assert_eq!(
        Dialect::resolve(Dialect::Auto, Dialect::Context, Path::new("a.tex")),
        Dialect::Context
    );
}

#[test]
fn dialect_defaults_include_list_verbatim_and_flat_environments() {
    let latex = Dialect::Latex.defaults();
    assert!(latex.lists.contains(&"itemize"));
    assert!(latex.verbatims.contains(&"verbatim"));
    assert!(latex.verbatims.contains(&"minted"));
    assert!(latex.no_indent_envs.contains(&"document"));
    let context = Dialect::Context.defaults();
    assert!(context.lists.contains(&"itemization") || context.lists.contains(&"itemize"));
    assert!(context.verbatims.contains(&"typing"));
    assert!(context.no_indent_envs.contains(&"text"));
    assert!(Dialect::Plain.defaults().lists.is_empty());
}

#[test]
fn every_name_and_alias_is_unique() {
    let mut names = Vec::new();
    for dialect in Dialect::ALL {
        names.push(dialect.name());
        names.extend(dialect.aliases().iter().copied());
    }
    names.sort();
    names.dedup();
    assert_eq!(
        names.len(),
        Dialect::ALL
            .iter()
            .map(|dialect| 1 + dialect.aliases().len())
            .sum::<usize>()
    );
}
