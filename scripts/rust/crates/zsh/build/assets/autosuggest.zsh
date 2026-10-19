# What zsh-autosuggestions' first binding reads: every widget, and its settings
_zsh_build_autosuggest_key() {
  emulate -L zsh
  local name
  # Hash order, alike in shells that defined the same widgets in the same order
  REPLY=${(pj:\n:)${(@kv)widgets}}
  for name in ${(ok)parameters[(I)(_|)ZSH_AUTOSUGGEST_*]}; do
    REPLY+=$'\n'$name:${parameters[$name]}=${(Pkvj:\0:)name}
  done
}
