if has_cmd nvim; then
  _search_files() {
    zle -I
    command nvim +TerminalSearch
    zle reset-prompt
  }
  _search_grep() {
    zle -I
    command nvim -c \
      'autocmd VimEnter * ++once Telescope live_grep'
    zle reset-prompt
  }

  zle -N search_files _search_files
  zle -N search_grep _search_grep

elif has_cmd fzf; then
  zle -A fzf-file-widget search_files
fi

if has_cmd atuin; then
  export ATUIN_NOBIND=true
  cached_eval atuin-init atuin init zsh

  zle -A atuin-search search_history
fi
