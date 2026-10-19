use super::*;

#[test]
fn a_job_writing_one_completion_file_names_its_function() {
    let job = "() {\n  local TMPPREFIX=\"$ZSH_CACHE_DIR/completions/._gh\"\n  zf_mv -f -- =( gh completion --shell zsh ) \"$ZSH_CACHE_DIR/completions/_gh\"\n} &|\n";
    assert_eq!(target(job).as_deref(), Some("gh"));
    let legacy = "if old; then\n  command cp x \"$ZSH_CACHE_DIR/completions/_docker\"\nelse\n  zf_mv -f -- =( docker completion zsh ) \"$ZSH_CACHE_DIR/completions/_docker\"\nfi &|\n";
    assert_eq!(target(legacy).as_deref(), Some("docker"));
}

#[test]
fn a_job_writing_other_files_or_several_is_not_taken() {
    for job in [
        "tool completion >| \"$HOME/_tool\" &|\n",
        "a >| \"$ZSH_CACHE_DIR/completions/_a\"; b >| \"$ZSH_CACHE_DIR/completions/_b\" &|\n",
        "a >| \"$ZSH_CACHE_DIR/completions/_$name\" &|\n",
    ] {
        assert_eq!(target(job), None, "{job}");
    }
}

#[test]
fn trailing_comments_are_not_part_of_the_job() {
    assert_eq!(
        without_trailing_comments("a &|\n\n# alias next\n  # more\n"),
        "a &|"
    );
    assert_eq!(without_trailing_comments("a &| # same line\n"), "a &| # same line");
}
