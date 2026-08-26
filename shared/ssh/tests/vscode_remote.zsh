#!/bin/zsh
emulate -LR zsh
setopt errexit nounset pipefail

typeset tests_dir=${0:A:h}
typeset repo_root=${tests_dir:h:h:h}
typeset source_file=$repo_root/shared/zsh/conf.d/44-vscode-remote.zsh
typeset opener=$repo_root/shared/ssh/bin/vscode-remote-open
typeset scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

fail() {
  print -u2 -r -- "vscode remote SSH test: $*"
  exit 1
}

mkdir -p -- "$scratch/bin" "$scratch/project with spaces"

cat >"$scratch/bin/ssh" <<'SH'
#!/bin/sh
printf '%s\n' "$@" >"$SSH_ARGS"
cat >"$SSH_INPUT"
SH
chmod +x "$scratch/bin/ssh"

cat >"$scratch/bin/code" <<'SH'
#!/bin/sh
printf '%s\n' "$@" >"$CODE_ARGS"
SH
chmod +x "$scratch/bin/code"

cat >"$scratch/bin/uname" <<'SH'
#!/bin/sh
printf '%s\n' "$TEST_UNAME"
SH
chmod +x "$scratch/bin/uname"

cat >"$scratch/bin/systemd-run" <<'SH'
#!/bin/sh
printf '%s\n' "$@" >"$SYSTEMD_ARGS"
SH
chmod +x "$scratch/bin/systemd-run"

typeset test_path="$scratch/project with spaces"
typeset ssh_args=$scratch/ssh.args
typeset ssh_input=$scratch/ssh.input
typeset code_args=$scratch/code.args
typeset systemd_args=$scratch/systemd.args
typeset test_path_physical=${test_path:A}
typeset -a args input

env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' TERM=xterm-ghostty \
  SSH_ARGS="$ssh_args" SSH_INPUT="$ssh_input" CODE_ARGS="$code_args" \
  /bin/zsh -fc 'source "$1"; builtin cd -- "$2"; code .' test "$source_file" "$test_path"

[[ -s "$ssh_args" && -s "$ssh_input" ]] || fail 'Ghostty SSH shell did not send a request back to Macie'
args=("${(@f)$(command cat -- "$ssh_args")}")
[[ "${(j:|:)args}" == '-o|BatchMode=yes|-o|ConnectTimeout=4|-T|macie|~/.ssh/bin/vscode-remote-open' ]] ||
  fail "unexpected SSH arguments: ${(j: :)args}"
input=("${(@f)$(command cat -- "$ssh_input")}")
[[ "$input[1]" == archie && "$input[2]" =~ '^[A-Za-z0-9+/]+={0,2}$' ]] || fail 'invalid Archie request envelope'
[[ "$(print -rn -- "$input[2]" | base64 -d)" == "$test_path_physical" ]] || fail 'Archie path was not preserved'

rm -f -- "$ssh_args" "$ssh_input"
env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=macie SSH_CONNECTION='client 1 server 2' TERM=wezterm \
  SSH_ARGS="$ssh_args" SSH_INPUT="$ssh_input" CODE_ARGS="$code_args" \
  /bin/zsh -fc 'source "$1"; builtin cd -- "$2"; code .' test "$source_file" "$test_path"
args=("${(@f)$(command cat -- "$ssh_args")}")
input=("${(@f)$(command cat -- "$ssh_input")}")
[[ "$args[6]" == archie && "$input[1]" == macie ]] || fail 'Macie request was not routed back to Archie'

rm -f -- "$code_args"
env -u SSH_CONNECTION -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=macie TERM=xterm-ghostty CODE_ARGS="$code_args" \
  /bin/zsh -fc 'source "$1"; code .' test "$source_file"
[[ "$(command cat -- "$code_args")" == . ]] || fail 'local invocation did not use native code'

env -u TMUX \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' TERM=xterm-ghostty \
  VSCODE_IPC_HOOK_CLI="$scratch/vscode.sock" CODE_ARGS="$code_args" \
  /bin/zsh -fc 'source "$1"; code .' test "$source_file"
[[ "$(command cat -- "$code_args")" == . ]] || fail 'VS Code terminal did not use native code'

env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=server SSH_CONNECTION='client 1 server 2' TERM=xterm-ghostty \
  CODE_ARGS="$code_args" /bin/zsh -fc 'source "$1"; code --version' test "$source_file"
[[ "$(command cat -- "$code_args")" == --version ]] || fail 'unrecognized host did not use native code'

[[ -x "$opener" ]] || fail 'client-side opener is missing'
typeset encoded=$(print -rn -- '/home/fredrir/project with spaces' | base64 | tr -d '\r\n')

rm -f -- "$code_args"
print -r -- "archie\n$encoded" | env \
  PATH="$scratch/bin:/usr/bin:/bin" TEST_UNAME=Darwin CODE_ARGS="$code_args" SYSTEMD_ARGS="$systemd_args" \
  "$opener"
args=("${(@f)$(command cat -- "$code_args")}")
[[ "${(j:|:)args}" == '--remote|ssh-remote+archie|/home/fredrir/project with spaces' ]] ||
  fail "macOS opener arguments were not preserved: ${(j: :)args}"

rm -f -- "$systemd_args"
print -r -- "macie\n$encoded" | env -u DISPLAY -u WAYLAND_DISPLAY \
  PATH="$scratch/bin:/usr/bin:/bin" TEST_UNAME=Linux CODE_ARGS="$code_args" SYSTEMD_ARGS="$systemd_args" \
  "$opener"
args=("${(@f)$(command cat -- "$systemd_args")}")
[[ "${(j:|:)args}" == "--user|--collect|--quiet|--|$scratch/bin/code|--remote|ssh-remote+macie|/home/fredrir/project with spaces" ]] ||
  fail "Linux opener arguments were not preserved: ${(j: :)args}"

rm -f -- "$code_args" "$systemd_args"
if print -r -- "other\n$encoded" | env \
  PATH="$scratch/bin:/usr/bin:/bin" TEST_UNAME=Darwin CODE_ARGS="$code_args" SYSTEMD_ARGS="$systemd_args" \
  "$opener" 2>/dev/null; then
  fail 'opener accepted an unrecognized host'
fi
[[ ! -e "$code_args" && ! -e "$systemd_args" ]] || fail 'invalid request launched a process'

print -r -- 'zsh routes through nested SSH and the client opener preserves validated arguments'
