if command -v nvim >/dev/null; then
	_terminal_telescope_widget() {
		zle -I
		command nvim +TerminalSearch
		zle reset-prompt
	}

	zle -N terminal-telescope-widget _terminal_telescope_widget
	bindkey -M emacs '^F' terminal-telescope-widget
	bindkey -M vicmd '^F' terminal-telescope-widget
	bindkey -M viins '^F' terminal-telescope-widget
fi

if command -v nvim >/dev/null; then
	_terminal_telescope_grep_widget() {
		zle -I

		command nvim -c \
			'autocmd VimEnter * ++once Telescope live_grep'

		zle reset-prompt
	}

	zle -N terminal-telescope-grep-widget _terminal_telescope_grep_widget

	bindkey -M emacs '^G' terminal-telescope-grep-widget
	bindkey -M vicmd '^G' terminal-telescope-grep-widget
	bindkey -M viins '^G' terminal-telescope-grep-widget
fi

if command -v atuin >/dev/null; then
	export ATUIN_NOBIND=true
	eval "$(atuin init zsh)"

	bindkey -M emacs '^R' atuin-search
	bindkey -M viins '^R' atuin-search-viins:q
	bindkey -M vicmd '^R' atuin-search-vicmd
fi

command -v fzf >/dev/null || return 0

export FZF_DEFAULT_OPTS="--height=40% --layout=reverse --border"

if ! command -v nvim >/dev/null; then
	bindkey -M emacs '^F' fzf-file-widget
	bindkey -M vicmd '^F' fzf-file-widget
	bindkey -M viins '^F' fzf-file-widget
fi

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
