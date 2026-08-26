# Let `code .` in an Archie/Macie SSH shell invoke VS Code on the paired
# client machine. Other invocations keep the native CLI behavior.
_vscode_remote_pair() {
  emulate -L zsh

  local host=${HOST:l}
  host=${host%%.*}
  case "$host" in
    archpc|archie) print -r -- 'archie macie' ;;
    macie) print -r -- 'macie archie' ;;
    *) return 1 ;;
  esac
}

_vscode_remote_open() {
  emulate -L zsh

  local remote_host=$1 client_host=$2 remote_path=$3 encoded
  (( $+commands[base64] && $+commands[tr] && $+commands[ssh] )) || {
    print -u2 -r -- 'code: base64, tr and ssh are required for remote opening'
    return 127
  }

  encoded=$(printf '%s' "$remote_path" |
    command base64 | command tr -d '\r\n') || return

  printf '%s\n%s\n' "$remote_host" "$encoded" |
    command ssh -o BatchMode=yes -o ConnectTimeout=4 -T \
      "$client_host" '~/.ssh/bin/vscode-remote-open'
}

code() {
  emulate -L zsh

  if (( $# == 1 )) && [[ "$1" == . && -n ${SSH_CONNECTION:-} && -z ${VSCODE_IPC_HOOK_CLI:-} ]]; then
    local pair remote_host client_host
    if pair=$(_vscode_remote_pair); then
      remote_host=${pair%% *}
      client_host=${pair#* }
      _vscode_remote_open "$remote_host" "$client_host" "${PWD:A}"
      return
    fi
  fi

  command code "$@"
}
