command -v fzf >/dev/null || return 0

export FZF_DEFAULT_OPTS="--height=40% --layout=reverse --border"

if command -v bat >/dev/null; then
  export FZF_CTRL_T_OPTS="--preview 'bat --color=always --style=numbers --line-range=:200 {} 2>/dev/null || ls --color=always {}'"
fi
_fzf_dir_preview='ls --color=always "$realpath"'
command -v eza >/dev/null && _fzf_dir_preview='eza --color=always --icons=auto "$realpath"'

zstyle ':completion:*' menu no
zstyle ':completion:*:descriptions' format '[%d]'
zstyle ':fzf-tab:*' switch-group '<' '>'
zstyle ':completion:*:git-checkout:*' sort false
zstyle ':fzf-tab:complete:cd:*' fzf-preview $_fzf_dir_preview
zstyle ':fzf-tab:complete:z:*' fzf-preview $_fzf_dir_preview
zstyle ':fzf-tab:*' fzf-flags '--height=60%'
unset _fzf_dir_preview
