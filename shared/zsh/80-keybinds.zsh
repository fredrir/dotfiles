bindkey -e
bindkey -N motion-select emacs

key() { ((${+widgets[${@[-1]}]})) && bindkey -M emacs "$@" && bindkey -M viins "$@"; }
vikey() { key "$@" && bindkey -M vicmd "$@"; }
selkey() { key "$@" && bindkey -M motion-select "$@"; }

vikey '^F' search_files
vikey '^G' search_grep
vikey '^H' search_history
vikey $'\e[115;9u' wezterm-open-yazi
vikey $'\e[5;30012~' wezterm-open-yazi

# Motion
key '^W' motion-backward-kill-shell-word
key '^U' motion-kill-to-line-start
key '^[^?' motion-backward-kill-word
key $'\e[1;5H' motion-document-start
key $'\e[1;5F' motion-document-end
key $'\e[13;2u' insert-newline

# Selection
selkey $'\e[1;2D' motion-select-backward-char
selkey $'\e[1;2C' motion-select-forward-char
selkey $'\e[1;2A' motion-select-backward-char
selkey $'\e[1;2B' motion-select-forward-char
selkey $'\e[1;4D' motion-select-backward-word
selkey $'\e[1;4C' motion-select-forward-word
selkey $'\e[1;2H' motion-select-beginning-of-line
selkey $'\e[1;2F' motion-select-end-of-line
selkey $'\e[1;6H' motion-select-buffer-start
selkey $'\e[1;6F' motion-select-buffer-end

select_key() { ((${+widgets[${@[-1]}]})) && bindkey -M motion-select "$@"; }

select_key -R '^@'-'\M-^?' motion-deselect
select_key -R ' '-'~' motion-replace-selection

select_key '^?' motion-kill-selection
select_key '^H' motion-kill-selection
select_key $'\e[3~' motion-kill-selection
