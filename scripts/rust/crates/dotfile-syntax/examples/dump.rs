//! Prints the lex and parse dumps for fixture authoring.
//!
//! Usage: `cargo run -p dotfile-syntax --example dump -- lex|parse|bootstrap [--json]`
//! reads the source on stdin. With `--json`, prints one object with the
//! `tokens`, `cst`, and `diagnostics` channels; otherwise prints readable
//! dumps.

use std::io::Read;

use dotfile_source::{RepoPath, SourceText, read_bootstrap};
use dotfile_syntax::{dump_tokens, lex, parse};

fn main() {
    let mut args = std::env::args().skip(1);
    let operation = args.next().unwrap_or_else(|| "parse".to_owned());
    let mut json = false;
    let mut path = "fixture.dotfile".to_owned();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--path" => path = args.next().unwrap_or(path),
            _ => {}
        }
    }
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    let path = RepoPath::new(&path).unwrap();
    let source = SourceText::from_bytes(input);
    match operation.as_str() {
        "lex" => {
            let lexed = lex(&path, &source);
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "tokens": lexed.dump(source.as_bytes()),
                        "diagnostics": serde_json::to_value(&lexed.diagnostics).unwrap(),
                    })
                );
            } else {
                print!("{}", lexed.dump(source.as_bytes()));
                print_diagnostics(&lexed.diagnostics);
            }
        }
        "parse" => {
            let result = parse(&path, &source);
            let tokens = dump_tokens(
                &result.cst.tokens,
                &result.cst.gaps,
                &result.cst.strings,
                source.as_bytes(),
            );
            let cst = result.cst.dump(source.as_bytes());
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "tokens": tokens,
                        "cst": cst,
                        "diagnostics": serde_json::to_value(&result.diagnostics).unwrap(),
                    })
                );
            } else {
                println!("{tokens}");
                println!("{cst}");
                print_diagnostics(&result.diagnostics);
            }
        }
        "bootstrap" => {
            let diagnostics = match read_bootstrap(&path, &source) {
                Ok(_) => Vec::new(),
                Err(errors) => errors,
            };
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "diagnostics": serde_json::to_value(&diagnostics).unwrap(),
                    })
                );
            } else {
                print_diagnostics(&diagnostics);
            }
        }
        other => {
            eprintln!("unknown operation {other}");
            std::process::exit(2);
        }
    }
}

fn print_diagnostics(diagnostics: &[dotfile_source::Diagnostic]) {
    println!("--- diagnostics ---");
    println!("{}", serde_json::to_string_pretty(diagnostics).unwrap());
}
