# Trim trailing CR/LF from a paste so a copied trailing newline doesn't leave the
# cursor on a blank line. Interior newlines (multi-line pastes) are kept.
# Hooks oh-my-zsh's bracketed-paste-magic paste-finish.
_strip_paste_trailing_newline() {
  emulate -L zsh
  setopt extendedglob
  PASTED=${PASTED%%[$'\r\n']##}
}
zstyle :bracketed-paste-magic paste-finish _strip_paste_trailing_newline
