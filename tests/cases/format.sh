#!/usr/bin/env bash

write_conf() {
  local path="$REPO/$1"
  mkdir -p "$(dirname "$path")"
  printf '%s' "$2" > "$path"
}

test_indents_hypr_blocks_and_normalises_assignments() {
  write_conf shared/hypr/hyprland.conf 'general {
key=value
nested {
a  =  b
}
}
'
  dotfile format "$REPO/shared/hypr/hyprland.conf"
  assert_ok
  assert_file_is "$REPO/shared/hypr/hyprland.conf" 'general {
    key = value
    nested {
        a = b
    }
}'
}

test_keeps_hypr_comments_unindented_as_written() {
  write_conf shared/hypr/hyprland.conf '# top
general {
# inner
key=value
}
'
  dotfile format "$REPO/shared/hypr/hyprland.conf"
  assert_ok
  assert_file_is "$REPO/shared/hypr/hyprland.conf" '# top
general {
    # inner
    key = value
}'
}

test_aligns_kitty_values_and_bindings_in_separate_columns() {
  write_conf shared/kitty/kitty.conf 'font_family      Foo
font_size 12
map ctrl+a bar
map ctrl+shift+b baz
'
  dotfile format "$REPO/shared/kitty/kitty.conf"
  assert_ok
  assert_file_is "$REPO/shared/kitty/kitty.conf" 'font_family  Foo
font_size    12
map ctrl+a        bar
map ctrl+shift+b  baz'
}

test_treats_generated_colour_files_as_plain() {
  write_conf shared/kitty/colors.conf 'foreground              #cdd6f4
background   #1e1e2e
'
  dotfile format "$REPO/shared/kitty/colors.conf"
  assert_ok
  assert_file_is "$REPO/shared/kitty/colors.conf" 'foreground              #cdd6f4
background   #1e1e2e'
}

test_treats_the_generated_kitty_font_file_as_plain() {
  write_conf shared/kitty/conf.d/fonts.conf 'font_family  Hack Nerd Font Mono
font_size    12
'
  dotfile format "$REPO/shared/kitty/conf.d/fonts.conf"
  assert_ok
  assert_file_is "$REPO/shared/kitty/conf.d/fonts.conf" 'font_family  Hack Nerd Font Mono
font_size    12'
}

test_collapses_blank_runs_and_trailing_whitespace() {
  write_conf shared/other/thing.conf 'one


two
'
  dotfile format "$REPO/shared/other/thing.conf"
  assert_ok
  assert_file_is "$REPO/shared/other/thing.conf" 'one

two'
}

test_reports_how_many_files_changed() {
  write_conf shared/other/a.conf 'needs


reflow
'
  write_conf shared/other/b.conf 'already fine
'
  dotfile format "$REPO/shared/other/a.conf" "$REPO/shared/other/b.conf"
  assert_ok
  assert_output_has "formatted 1 of 2"
}

test_formats_every_conf_under_a_directory() {
  write_conf shared/other/a.conf 'a


a
'
  write_conf shared/other/nested/b.conf 'b


b
'
  write_conf shared/other/skip.txt 'c
'
  dotfile format "$REPO/shared/other"
  assert_ok
  assert_output_has "formatted 2 of 2"
  assert_file_is "$REPO/shared/other/skip.txt" 'c'
}

test_rejects_files_that_are_not_conf() {
  write_conf shared/other/thing.txt 'x
'
  dotfile format "$REPO/shared/other/thing.txt"
  assert_fails
  assert_output_has "not a .conf file"
}
