bindkey -N fredrir emacs
bindkey -A fredrir main

key() { ((${+widgets[${@[-1]}]})) && bindkey -M fredrir "$@"; }

key '^F' search_files
key '^G' search_grep
key '^H' search_history

# Motion

key '^W' motion-backward-kill-shell-word
key '^U' motion-kill-to-line-start
key '^[^?' motion-backward-kill-word

key -R '^@'-'\M-^?' motion-deselect
key -R ' '-'~' motion-replace-selection

key '^?' motion-kill-selection
key '^H' motion-kill-selection
key $'\e[3~' motion-kill-selection

key $'\e[1;5H' motion-document-start
key $'\e[1;5F' motion-document-end
key $'\e[1;2D' motion-select-backward-char
key $'\e[1;2C' motion-select-forward-char
key $'\e[1;2A' motion-select-backward-char
key $'\e[1;2B' motion-select-forward-char
key $'\e[1;4D' motion-select-backward-word
key $'\e[1;4C' motion-select-forward-word
key $'\e[1;2H' motion-select-beginning-of-line
key $'\e[1;2F' motion-select-end-of-line
key $'\e[1;6H' motion-select-buffer-start
key $'\e[1;6F' motion-select-buffer-end

key $'\e[13;2u' insert-newline
