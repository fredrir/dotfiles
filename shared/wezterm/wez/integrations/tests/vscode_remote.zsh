#!/bin/zsh
emulate -LR zsh
setopt errexit nounset pipefail

typeset tests_dir=${0:A:h}
typeset repo_root=${tests_dir:h:h:h:h:h}
typeset source_file=$repo_root/shared/zsh/conf.d/44-vscode-remote.zsh
typeset scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

fail() {
  print -u2 -r -- "vscode remote zsh test: $*"
  return 1
}

mkdir -p -- "$scratch/bin" "$scratch/project with spaces"

cat >"$scratch/bin/code" <<'SH'
#!/bin/sh
printf native
for arg do
  printf '|%s' "$arg"
done
SH
chmod +x "$scratch/bin/code"

cat >"$scratch/bin/tmux" <<'SH'
#!/bin/sh
if [ "$#" -eq 3 ] && [ "$1" = display-message ] && [ "$2" = -p ] && [ "$3" = '#{client_termname}' ]; then
  printf 'wezterm\n'
  exit 0
fi
exit 1
SH
chmod +x "$scratch/bin/tmux"

decode_request() {
  local raw=$1 prefix suffix encoded
  prefix=$'\e]1337;SetUserVar=vscode_remote_open='
  suffix=$'\a'
  [[ "$raw" == "$prefix"*"$suffix" ]] || fail 'direct request is not a complete OSC 1337 sequence'
  encoded=${raw#"$prefix"}
  encoded=${encoded%"$suffix"}
  print -rn -- "$encoded" | base64 -d
}

decode_tmux_request() {
  local raw=$1 prefix suffix encoded
  prefix=$'\ePtmux;\e\e]1337;SetUserVar=vscode_remote_open='
  suffix=$'\a\e\\'
  [[ "$raw" == "$prefix"*"$suffix" ]] || fail 'tmux request is not a passthrough OSC 1337 sequence'
  encoded=${raw#"$prefix"}
  encoded=${encoded%"$suffix"}
  print -rn -- "$encoded" | base64 -d
}

assert_payload() {
  local decoded=$1 expected_host=$2 expected_path=$3 rest host nonce path
  host=${decoded%%$'\n'*}
  rest=${decoded#*$'\n'}
  nonce=${rest%%$'\n'*}
  path=${rest#*$'\n'}

  [[ "$host" == "$expected_host" ]] || fail "expected host $expected_host, got $host"
  [[ "$nonce" == <->.<-> ]] || fail "invalid invocation nonce: $nonce"
  [[ "$path" == "$expected_path" ]] || fail "expected path $expected_path, got $path"
}

typeset test_path="$scratch/project with spaces"
typeset request decoded
request=$(env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' TERM=wezterm \
  /bin/zsh -fc 'source "$1"; builtin cd -- "$2"; code .' test "$source_file" "$test_path")
decoded=$(decode_request "$request")
assert_payload "$decoded" archie "$test_path"

request=$(env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=macie SSH_CONNECTION='client 1 server 2' TERM=wezterm \
  /bin/zsh -fc 'source "$1"; builtin cd -- "$2"; code .' test "$source_file" "$test_path")
decoded=$(decode_request "$request")
assert_payload "$decoded" macie "$test_path"

request=$(env -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' \
  TERM=tmux-256color TMUX="$scratch/tmux.sock" \
  /bin/zsh -fc 'source "$1"; builtin cd -- "$2"; code .' test "$source_file" "$test_path")
decoded=$(decode_tmux_request "$request")
assert_payload "$decoded" archie "$test_path"

typeset native
native=$(env -u SSH_CONNECTION -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=macie TERM=wezterm \
  /bin/zsh -fc 'source "$1"; code .' test "$source_file")
[[ "$native" == 'native|.' ]] || fail "local shell did not use native code: $native"

native=$(env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' TERM=xterm-256color \
  /bin/zsh -fc 'source "$1"; code .' test "$source_file")
[[ "$native" == 'native|.' ]] || fail "non-WezTerm SSH shell did not use native code: $native"

native=$(env -u TMUX \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' \
  TERM=wezterm VSCODE_IPC_HOOK_CLI="$scratch/vscode.sock" \
  /bin/zsh -fc 'source "$1"; code .' test "$source_file")
[[ "$native" == 'native|.' ]] || fail "VS Code terminal did not use native code: $native"

native=$(env -u TMUX -u VSCODE_IPC_HOOK_CLI \
  PATH="$scratch/bin:/usr/bin:/bin" HOST=archpc SSH_CONNECTION='client 1 server 2' TERM=wezterm \
  /bin/zsh -fc 'source "$1"; code --version' test "$source_file")
[[ "$native" == 'native|--version' ]] || fail "non-dot invocation did not use native code: $native"

print -r -- 'zsh emits direct/tmux requests and preserves native fallbacks'

