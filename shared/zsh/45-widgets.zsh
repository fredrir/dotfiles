if has_cmd nvim; then
  _search_files() {
    command nvim +TerminalSearch
  }
  _search_grep() {
    command nvim -c \
      'autocmd VimEnter * ++once Telescope live_grep'
  }

  zle -N search_files _search_files
  zle -N search_grep _search_grep
fi

if has_cmd atuin; then
  export ATUIN_NOBIND=true
  eval "$(atuin init zsh)"

  zle -A atuin-search search_history
fi
