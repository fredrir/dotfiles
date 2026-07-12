_strip_paste_trailing_newline() {
  emulate -L zsh
  setopt extendedglob
  PASTED=${PASTED%%[$'\r\n']##}
}
zstyle :bracketed-paste-magic paste-finish _strip_paste_trailing_newline
