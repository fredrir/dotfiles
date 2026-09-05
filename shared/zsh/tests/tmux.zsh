# Run from the repository root: zsh -dfi shared/zsh/tests/tmux.zsh
unset WEZTERM_PANE TMUX
source shared/zsh/conf.d/94-tmux.zsh
[[ $(_tmux_owner_reset) == $'\e]1337;SetUserVar=TMUX_WORKSPACE=\a' ]] || exit 1
source shared/zsh/conf.d/49-wezterm.zsh
[[ $(bindkey -M emacs $'\e[13;2u') == *wezterm-insert-newline ]] || exit 1
[[ $(bindkey -M viins $'\e[115;9u') == *wezterm-open-yazi ]] || exit 1
[[ $(bindkey -M viins $'\e[5;30012~') == *wezterm-open-yazi ]] || exit 1
LBUFFER=before
_wezterm_insert_newline
[[ $LBUFFER == $'before\n' ]] || exit 1

typeset -a dispatched
tmux-workspace() { dispatched=("$@"); }
TMUX=/tmp/isolated-test,123,1
TMUX_PANE=%17
attach_mux macie
[[ ${(j: :)dispatched} == 'host macie --pane %17' ]] || exit 1
source shared/zsh/conf.d/94-tmux.zsh
t 'project with spaces'
[[ $#dispatched == 2 && $dispatched[1] == enter && $dispatched[2] == 'project with spaces' ]] || exit 1
source shared/zsh/conf.d/94-tmux.zsh
[[ ${#${(M)precmd_functions:#_tmux_prompt_start}} == 1 ]] || exit 1
[[ ${#${(M)preexec_functions:#_tmux_command_start}} == 1 ]] || exit 1
[[ $(_tmux_command_start) == $'\e]133;C\a' ]] || exit 1
[[ $(_tmux_prompt_end) == $'\e]133;B\a' ]] || exit 1
_tmux_command_running=1
result=$(false; _tmux_prompt_start)
[[ $result == $'\e]133;D;1\a\e]133;A\a' ]] || exit 1
add-zsh-hook -d preexec _tmux_command_start
add-zsh-hook -d precmd _tmux_prompt_start

# The popup must return its directory to the originating shell and remove the
# temporary handoff file, including paths returned without a trailing newline.
local original_cwd=$PWD result_file
hash tmux-workspace=/bin/true
tmux-workspace() {
  [[ $1 == yazi && $2 == --pane && $3 == %17 && $4 == --cwd-file ]] || return 1
  result_file=$5
  print -rn -- /tmp > "$result_file"
}
zle() { return 0; }
_wezterm_open_yazi || exit 1
[[ $PWD == /tmp && ! -e $result_file ]] || exit 1
builtin cd -- "$original_cwd"
print -r -- 'tmux shell integration: passed'
