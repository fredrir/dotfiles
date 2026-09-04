use super::*;

#[test]
fn a_plain_style_paints_nothing() {
    let style = Style::plain();
    assert_eq!(style.bold("gdd"), "gdd");
    assert_eq!(style.teal("~/dotfiles"), "~/dotfiles");
}

#[test]
fn painting_wraps_the_text_it_is_given() {
    let style = Style {
        colored: true,
        green: "\x1b[32m".into(),
        red: String::new(),
        teal: String::new(),
    };
    assert_eq!(style.green("+2"), "\x1b[32m+2\x1b[0m");
    assert_eq!(style.green(""), "");
}
