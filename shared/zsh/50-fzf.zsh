if has_cmd fzf; then
  export FZF_DEFAULT_OPTS="--height=40% --layout=reverse --border"

  has_cmd bat &&
    export FZF_CTRL_T_OPTS="--preview 'bat --color=always --style=numbers --line-range=:200 {} 2>/dev/null || ls --color=always {}'"

  _fzf_dir_preview='ls --color=always "$realpath"'

  if has_cmd eza; then
    _fzf_dir_preview='eza --color=always --icons=auto "$realpath"'
    zstyle ':completion:*' list-colors "${(@)${(@s.:.)EZA_COLORS}:#reset}"
  fi

  zstyle ':completion:*' menu no
  zstyle ':completion:*:descriptions' format '[%d]'
  zstyle ':completion:*:git-checkout:*' sort false

  zstyle ':fzf-tab:complete:cd:*' fzf-preview $_fzf_dir_preview
  zstyle ':fzf-tab:complete:z:*' fzf-preview $_fzf_dir_preview
  zstyle ':fzf-tab:*' fzf-flags '--height=60%'
  zstyle ':fzf-tab:complete:ga:argument-rest' default-color "${THEME_GIT}"
  zstyle ':fzf-tab:*' switch-group '<' '>'

  unset _fzf_dir_preview
fi
