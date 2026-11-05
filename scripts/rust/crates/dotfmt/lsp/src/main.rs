use std::process::ExitCode;

fn main() -> ExitCode {
    match dotfmt_lsp::serve() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dotfmt-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
