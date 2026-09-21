bindkey -e

bindkey -N motion-select

key() { ((${+widgets[${@[-1]}]})) && bindkey -M emacs "$@" && bindkey -M viins "$@"; }
vikey() { key "$@" && bindkey -M vicmd "$@"; }
selkey() { ((${+widgets[${@[-1]}]})) && bindkey -M motion-select "$@"; }
selectkey() { key "$@" && selkey "$@"; }

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
key $'\e[H' beginning-of-line
key $'\e[F' end-of-line
key $'\e[1;5D' backward-word
key $'\e[1;5C' forward-word
key $'\e[13;2u' insert-newline
key $'\e[99;9u' motion-copy-selection

selkey -R '^@'-'\M-^?' motion-deselect
selkey -R ' '-'~' motion-replace-selection
selkey -R '\M-^@'-'\M-^?' motion-replace-selection

selectkey $'\e[1;2D' motion-select-backward-char
selectkey $'\e[1;2C' motion-select-forward-char
selectkey $'\e[1;2A' motion-select-up-line
selectkey $'\e[1;2B' motion-select-down-line
selectkey $'\e[1;6D' motion-select-backward-word
selectkey $'\e[1;6C' motion-select-forward-word
selectkey $'\e[1;4D' motion-select-backward-word
selectkey $'\e[1;4C' motion-select-forward-word
selectkey $'\e[1;2H' motion-select-beginning-of-line
selectkey $'\e[1;2F' motion-select-end-of-line
selectkey $'\e[1;6H' motion-select-buffer-start
selectkey $'\e[1;6F' motion-select-buffer-end

selkey $'\e[99;9u' motion-copy-selection
selkey $'\e[200~' motion-replace-selection

for sequence in '^?' '^D' '^K' '^U' '^W' '^[^?' '^[d' '^[[3~'; do
  selkey "$sequence" motion-kill-selection
done
unset sequence
