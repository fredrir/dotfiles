use super::*;

#[test]
fn groups_render_as_tab_separated_lines() {
    let mut reply = Reply::new();
    reply.skip("--tools=read,");
    reply.group(
        Group::new("tools", "tool")
            .unsorted()
            .suffix(",", true)
            .insert_prefix("x:")
            .replace()
            .items([Item::new("bash", "built-in"), Item::bare("edit")]),
    );
    reply.files();
    assert_eq!(
        reply.plain(),
        "skip\t--tools=read,\n\
         group\ttools\ttool\tlines\tunsorted\tsuffix=,\tremovable\treplace\tprefix=x:\n\
         item\tbash\tbash  built-in\n\
         item\tedit\tedit\n\
         files\n"
    );
}

#[test]
fn a_value_is_offered_once_across_groups() {
    let mut reply = Reply::new();
    reply.group(Group::new("history", "recent").items([Item::bare("react"), Item::bare("vue")]));
    reply.group(
        Group::new("registry", "registry").items([Item::bare("react"), Item::bare("preact")]),
    );
    assert_eq!(reply.values(), ["react", "vue", "preact"]);
}

#[test]
fn empty_groups_are_left_out() {
    let mut reply = Reply::new();
    reply.group(Group::new("none", "nothing"));
    reply.group(Group::new("blank", "blank").items([Item::bare("")]));
    assert_eq!(reply.plain(), "");
}

#[test]
fn sections_after_delegate_follow_it() {
    let mut reply = Reply::new();
    reply.message("before");
    reply.delegate();
    reply.group(Group::new("commands", "command").items([Item::bare("install")]));
    assert_eq!(
        reply.plain(),
        "message\tbefore\ndelegate\ngroup\tcommands\tcommand\nitem\tinstall\tinstall\n"
    );
}

#[test]
fn control_characters_cannot_break_the_format() {
    let mut reply = Reply::new();
    reply.group(Group::new("t", "l").items([Item::new("a\tb", "line one\nline two")]));
    assert!(
        reply
            .plain()
            .contains("item\ta b\ta b  line one line two\n"),
        "{}",
        reply.plain()
    );
}

#[test]
fn long_descriptions_are_shortened_at_a_word() {
    assert_eq!(shorten("short", 10), "short");
    assert_eq!(shorten("a description that runs on", 12), "a descripti…");
    assert_eq!(shorten("abc def ghi", 5), "abc…");
}

#[test]
fn details_line_up_in_columns_even_when_one_is_missing() {
    let mut reply = Reply::new();
    reply.group(
        Group::new("registry", "package").items([
            Item::bare("react")
                .detail(Tone::Info, "647M/mo")
                .detail(Tone::Muted, "UI"),
            Item::bare("rea")
                .detail(Tone::Info, "")
                .detail(Tone::Muted, "router"),
        ]),
    );
    assert_eq!(
        reply.plain(),
        "group\tregistry\tpackage\tlines\n\
         item\treact\treact  647M/mo  UI\n\
         item\trea\trea             router\n"
    );
}

#[test]
fn colors_come_from_the_palette_roles() {
    let mut reply = Reply::new();
    reply.group(Group::new("t", "l").items([Item::new("name", "about")]));
    let style = Style::for_mode(ui_theme::ColorMode::Always, true);
    let rendered = reply.render(&style);
    assert!(rendered.contains("\x1b["), "{rendered:?}");
    assert!(
        rendered.starts_with("group\tt\tl\tlines\nitem\tname\t"),
        "the value itself stays plain"
    );
}

#[test]
fn scoped_names_color_the_scope_apart_from_the_name() {
    let style = Style::for_mode(ui_theme::ColorMode::Always, true);
    let paint = |tone: Tone, text: &str| style.paint(tone.role(), text);
    let marker = style.code(
        &style
            .palette()
            .named_color("colors", "bright_green")
            .expect("palette bright_green")
            .sgr(false),
        "@",
    );
    assert_eq!(
        paint_name(&style, Tone::Accent, "@anthropic-ai/claude-code"),
        [
            paint(Tone::Accent, ""),
            marker.clone(),
            paint(Tone::Info, "anthropic-ai"),
            paint(Tone::Muted, "/"),
            paint(Tone::Accent, "claude-code"),
        ]
        .concat()
    );
    assert!(
        paint_name(&style, Tone::Accent, "npm:@upstash/context7-pi")
            .contains(&format!("{marker}{}", paint(Tone::Info, "upstash")))
    );
    for plain in ["react", "@types", "@/x", "deepseek/", "a/@b/c"] {
        assert_eq!(
            paint_name(&style, Tone::Accent, plain),
            paint(Tone::Accent, plain),
            "{plain}"
        );
    }
}
