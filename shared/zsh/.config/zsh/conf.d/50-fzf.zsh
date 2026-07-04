command -v fzf >/dev/null || return 0

export FZF_DEFAULT_OPTS="--height=40% --layout=reverse --border"

if command -v bat >/dev/null; then
  export FZF_CTRL_T_OPTS="--preview 'bat --color=always --style=numbers --line-range=:200 {} 2>/dev/null || ls --color=always {}'"
fi
_fzf_dir_preview='ls --color=always "$realpath"'
command -v eza >/dev/null && _fzf_dir_preview='eza --color=always --icons=auto "$realpath"'

# fzf-tab
zstyle ':completion:*' menu no
# Group headers (e.g. "local branch" vs "remote branch") shown inside fzf;
# switch between groups with < and >.
zstyle ':completion:*:descriptions' format '[%d]'
zstyle ':fzf-tab:*' switch-group '<' '>'
# Keep git branch completion in recency order instead of alphabetical.
zstyle ':completion:*:git-checkout:*' sort false
# Preview directory contents when completing cd/z arguments.
zstyle ':fzf-tab:complete:cd:*' fzf-preview $_fzf_dir_preview
zstyle ':fzf-tab:complete:z:*' fzf-preview $_fzf_dir_preview
zstyle ':fzf-tab:*' fzf-flags '--height=60%'
unset _fzf_dir_preview
