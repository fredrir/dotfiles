use super::*;

#[test]
fn truncating_the_front_keeps_the_end_that_identifies_the_path() {
    assert_eq!(truncate_front("sub/x/report.txt", 40), "sub/x/report.txt");
    assert_eq!(truncate_front("sub/x/report.txt", 12), "…/report.txt");
    assert_eq!(truncate_front("sub/x/report.txt", 12).chars().count(), 12);
    assert_eq!(truncate_front("short", 10), "short");
    assert_eq!(truncate_front("0123456789", 5), "…6789");
    assert_eq!(truncate_front("0123456789", 10), "0123456789");
}

#[test]
fn truncating_the_front_counts_characters_rather_than_bytes() {
    assert_eq!(truncate_front("émigré", 6), "émigré");
    assert_eq!(truncate_front("émigré", 3), "…ré");
}

#[test]
fn truncating_the_back_keeps_the_start_and_marks_the_cut() {
    assert_eq!(truncate_back("my-app", 10), "my-app");
    assert_eq!(truncate_back("my-app", 6), "my-app");
    assert_eq!(truncate_back("my-application", 6), "my-ap…");
    assert_eq!(truncate_back("my-app", 1), "…");
    assert_eq!(truncate_back("my-app", 0), "");
}

#[test]
fn truncating_the_back_counts_characters_rather_than_bytes() {
    assert_eq!(truncate_back("émigré", 6), "émigré");
    assert_eq!(truncate_back("émigré", 3), "ém…");
}

#[test]
fn a_plural_picks_the_word_the_count_calls_for() {
    assert_eq!(plural(1, "file", "files"), "file");
    assert_eq!(plural(0, "file", "files"), "files");
    assert_eq!(plural(2, "file", "files"), "files");
    assert_eq!(plural(1, "directory", "directories"), "directory");
    assert_eq!(plural(3, "directory", "directories"), "directories");
    assert_eq!(plural(1, "change", "changes"), "change");
    assert_eq!(plural(2, "entry", "entries"), "entries");
}

#[test]
fn a_counted_word_carries_the_count_in_front_of_it() {
    assert_eq!(counted(1, "file", "files"), "1 file");
    assert_eq!(counted(0, "file", "files"), "0 files");
    assert_eq!(counted(3, "file", "files"), "3 files");
    assert_eq!(counted(1, "directory", "directories"), "1 directory");
    assert_eq!(counted(4, "directory", "directories"), "4 directories");
}
