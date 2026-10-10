use super::*;

const CLAP: &str = r#"#compdef tool

autoload -U is-at-least

_tool() {
    _arguments '--help[Print help]' ":: :_tool_commands"
}

(( $+functions[_tool_commands] )) ||
_tool_commands() {
    local commands; commands=()
    _describe -t commands 'tool commands' commands "$@"
}

if [ "$funcstack[1]" = "_tool" ]; then
    _tool "$@"
else
    compdef _tool tool
fi
"#;

const COBRA: &str = r#"#compdef other
compdef _other other

# zsh completion for other
__other_debug()
{
    :
}

_other()
{
    __other_debug
}

# don't run the completion function when being source-ed or eval-ed
if [ "$funcstack[1]" = "_other" ]; then
    _other
fi
"#;

fn script(text: &str) -> Script {
    Script::parse(text.to_string()).unwrap()
}

#[test]
fn clap_and_cobra_scripts_are_autoloadable() {
    assert_eq!(autoloadable(&script(CLAP)).as_deref(), Some("_tool"));
    assert_eq!(autoloadable(&script(COBRA)).as_deref(), Some("_other"));
    let renamed = CLAP
        .replace("#compdef tool", "#compdef t")
        .replace("compdef _tool tool", "compdef _tool t");
    assert_eq!(autoloadable(&script(&renamed)).as_deref(), Some("_tool"));
}

#[test]
fn anything_else_is_not() {
    let cases = [
        CLAP.replace("#compdef tool\n", ""),
        CLAP.replace("#compdef tool", "#compdef -p tool"),
        CLAP.replace("compdef _tool tool", "compdef _tool tool other"),
        CLAP.replace("compdef _tool tool", "compdef _other tool"),
        CLAP.replace("_tool() {", "_tool2() {"),
        CLAP.replace("autoload -U is-at-least", "typeset -gA cache"),
        format!("{CLAP}typeset -gA after\n"),
        format!("{CLAP}compdef _tool tool\n"),
        CLAP.replace("then\n    _tool \"$@\"", "then\n    _other \"$@\""),
        CLAP.replace("[ \"$funcstack[1]\" = \"_tool\" ]", "[[ -n $x ]]"),
        COBRA.replace("compdef _other other", "zstyle ':completion:*' menu yes"),
        COBRA
            .replace("#compdef other", "#compdef ../other")
            .replace("compdef _other other", ":"),
    ];
    for text in cases {
        assert_eq!(autoloadable(&script(&text)), None, "{text}");
    }
}

#[test]
fn scripts_are_taken_only_once_the_directory_is_on_fpath() {
    let dir = tempfile::tempdir().unwrap();
    let mut completions = Completions::new(dir.path().join("zshrc.completions"));
    assert!(completions.take(&script(CLAP)).is_none());
    let fpath = completions.place();
    assert_eq!(
        fpath,
        format!(
            "fpath=({} \"${{fpath[@]}}\")\n",
            dir.path().join("zshrc.completions").display()
        )
    );
    let (file, functions) = completions.take(&script(CLAP)).unwrap();
    assert_eq!(file, dir.path().join("zshrc.completions/_tool"));
    assert_eq!(
        functions,
        vec![("_tool".to_string(), 5), ("_tool_commands".to_string(), 9)]
    );
    assert!(completions.take(&script("print hi\n")).is_none());
    assert!(Completions::default().place().is_empty());
}

#[test]
fn writing_replaces_the_directory_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zshrc.completions");
    let produce = |texts: &[&str]| {
        let mut completions = Completions::new(path.clone());
        completions.place();
        for text in texts {
            completions.take(&script(text)).unwrap();
        }
        completions
    };

    let first = produce(&[CLAP, COBRA]);
    assert_eq!(
        first.write(true).unwrap(),
        Written {
            changed: true,
            bindings: true
        }
    );
    assert!(!path.exists());
    first.write(false).unwrap();
    assert_eq!(fs::read_to_string(path.join("_tool")).unwrap(), CLAP);
    assert_eq!(first.write(false).unwrap(), Written::default());

    let body = CLAP.replace("Print help", "Print the help");
    let changed = produce(&[&body, COBRA]);
    assert_eq!(
        changed.write(false).unwrap(),
        Written {
            changed: true,
            bindings: false
        }
    );

    let rebound = CLAP
        .replace("#compdef tool", "#compdef tool t2")
        .replace("compdef _tool tool", "compdef _tool tool t2");
    let removed = produce(&[&rebound]);
    assert_eq!(
        removed.write(true).unwrap(),
        Written {
            changed: true,
            bindings: true
        }
    );
    assert!(path.join("_other").exists());
    removed.write(false).unwrap();
    assert!(!path.join("_other").exists());
    assert_eq!(fs::read_to_string(path.join("_tool")).unwrap(), rebound);
}
