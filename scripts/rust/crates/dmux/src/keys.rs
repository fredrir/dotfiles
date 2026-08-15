//! The live key bindings, never a hand-maintained copy.
//!
//! `wezterm show-keys` and `tmux list-keys -N` are the running programs'
//! own answers, so this output cannot drift from the configs the way a
//! cheat sheet would. `--man` wraps the same dumps in a roff page and execs
//! `man` on it, which buys paging, searching and nvim's Man mode for free.

use std::process::{Command, ExitCode, Stdio};

use workstation::Style;

use crate::PROGRAM;
use crate::attach;

pub fn run(man: bool, only_tmux: bool, only_wez: bool) -> Result<ExitCode, String> {
    let include_wez = only_wez || !only_tmux;
    let include_tmux = only_tmux || !only_wez;
    let mut sections = Vec::new();
    if include_wez {
        sections.push(("WEZTERM", dump("wezterm", &["show-keys"])));
    }
    if include_tmux {
        sections.push(("TMUX", dump("tmux", &["list-keys", "-N"])));
    }
    if man {
        return man_page(&sections);
    }
    let style = Style::for_stdout();
    let mut printed = false;
    for (title, text) in &sections {
        match text {
            Ok(text) => {
                if printed {
                    println!();
                }
                println!("{}", style.bold(&style.teal(&title.to_lowercase())));
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
                printed = true;
            }
            Err(message) => eprintln!("{PROGRAM}: {message}"),
        }
    }
    if printed {
        Ok(ExitCode::SUCCESS)
    } else {
        Err("no key bindings to show".to_string())
    }
}

fn dump(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} {} failed", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn man_page(sections: &[(&str, Result<String, String>)]) -> Result<ExitCode, String> {
    let mut roff = String::from(
        ".TH \"DMUX-KEYS\" \"1\" \"\" \"dmux\" \"dmux\"\n\
         .SH NAME\n\
         dmux-keys \\- live wezterm and tmux key bindings\n",
    );
    for (title, text) in sections {
        roff.push_str(&format!(".SH {title}\n.nf\n"));
        match text {
            Ok(text) => roff.push_str(&verbatim(text)),
            Err(message) => roff.push_str(&verbatim(&format!("unavailable: {message}\n"))),
        }
        roff.push_str(".fi\n");
    }
    let path = std::env::temp_dir().join(format!("dmux-keys-{}.1", std::process::id()));
    std::fs::write(&path, roff).map_err(|error| format!("{}: {error}", path.display()))?;
    let path = path
        .into_os_string()
        .into_string()
        .map_err(|_| "man page path is not unicode".to_string())?;
    // BSD man takes a path as the page name; man-db needs -l to read a file.
    let plan = match std::env::consts::OS {
        "linux" => vec!["man".to_string(), "-l".to_string(), path],
        _ => vec!["man".to_string(), path],
    };
    let mut envs = Vec::new();
    if attach::on_path("nvim") {
        envs.push(("MANPAGER", "nvim +Man!".to_string()));
    }
    Ok(attach::exec_plan(plan, &envs))
}

/// Verbatim text inside `.nf`: escape roff's escape character and defang
/// lines that would otherwise read as requests.
fn verbatim(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let escaped = line.replace('\\', "\\\\");
        if escaped.starts_with('.') || escaped.starts_with('\'') {
            out.push_str("\\&");
        }
        out.push_str(&escaped);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_defuses_roff_syntax() {
        assert_eq!(verbatim(".TH boom\n"), "\\&.TH boom\n");
        assert_eq!(verbatim("'quote\n"), "\\&'quote\n");
        assert_eq!(verbatim("C-\\ split\n"), "C-\\\\ split\n");
        assert_eq!(verbatim("plain\n"), "plain\n");
    }
}
