_normalize_paste() {
  emulate -L zsh
  setopt extendedglob

  if [[ -z $BUFFER ]]; then
    PASTED=${PASTED##[ $'\t']##}
  fi

  PASTED=${PASTED%%[$'\r\n']##}
}
zstyle :bracketed-paste-magic paste-finish _normalize_paste
