use super::*;

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(str::to_string).collect()
}

#[test]
fn rm_runs_only_when_a_file_is_there() {
    assert_eq!(
        remove(&words(r#"command rm -f "${ZSH_CACHE_DIR:-$ZSH/cache}/npm""#), true).as_deref(),
        Some(
            "[[ ! -e \"${ZSH_CACHE_DIR:-$ZSH/cache}/npm\" && ! -L \"${ZSH_CACHE_DIR:-$ZSH/cache}/npm\" ]] || command rm -f \"${ZSH_CACHE_DIR:-$ZSH/cache}/npm\"\n"
        )
    );
    assert_eq!(
        remove(&words("command rm -rf -- /tmp/a /tmp/b"), true).as_deref(),
        Some("[[ ! -e /tmp/a && ! -L /tmp/a && ! -e /tmp/b && ! -L /tmp/b ]] || command rm -rf -- /tmp/a /tmp/b\n")
    );
}

#[test]
fn rm_whose_words_expand_otherwise_in_a_test_is_kept() {
    for text in [
        "command rm /tmp/a",
        "command rm -i /tmp/a",
        "command rm -f",
        "command rm -f /tmp/*.log",
        "command rm -f $files",
        "command rm -f \"$(tmpfile)\"",
        "command rm -f \"${files[@]}\"",
        "command rm -f =tmp",
        "command rm -f ~/a",
        "rm -f /tmp/a",
    ] {
        assert_eq!(remove(&words(text), true), None, "{text}");
    }
    assert_eq!(remove(&words("command rm -f /tmp/a"), false), None);
}
