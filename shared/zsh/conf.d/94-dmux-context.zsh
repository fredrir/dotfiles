# Disable aliases before zsh parses any function bodies below. A caller may
# legitimately have aliases named `export`, `unset`, or even `always`; none
# may rewrite security-sensitive prompt code while this file is sourced.
_DMUX_CONTEXT_SOURCE_ALIASES=${options[aliases]:-off}
\builtin setopt noaliases

# Refresh dmux's pane marker at each prompt. The bootstrap marker is only a
# locator hint: `_context` revalidates it against the owner registry and a
# live provider scan before anything here is exported or sent to WezTerm.
autoload -Uz add-zsh-hook
add-zsh-hook -d precmd _dmux_context_refresh 2>/dev/null

# The managed presentation path is deliberately inert unless its rollout
# flag was inherited by this shell. In particular, flag-off shells retain
# their existing environment and emit no terminal control sequences.
if [[ ${DMUX_WEZ_FIRST:-0} != 1 ]]; then
  if [[ $_DMUX_CONTEXT_SOURCE_ALIASES == on ]]; then
    builtin unset _DMUX_CONTEXT_SOURCE_ALIASES
    builtin setopt aliases
  else
    builtin unset _DMUX_CONTEXT_SOURCE_ALIASES
  fi
  return 0
fi

zmodload zsh/datetime 2>/dev/null || true

typeset -g _DMUX_CONTEXT_REFRESH_ACTIVE=0
typeset -gF _DMUX_CONTEXT_REFRESHED_AT=0
typeset -g _DMUX_CONTEXT_REFRESH_SIGNATURE=
typeset -ga _DMUX_CONTEXT_REFRESH_B64=()
# `+x` deliberately strips an inherited export attribute. This locator is
# private shell state and is exposed only to one `_context` invocation below.
typeset -g +x _DMUX_CONTEXT_SPACE_UID_HINT=${DMUX_SPACE_UID:-${_DMUX_CONTEXT_SPACE_UID_HINT:-}}
typeset -gi _DMUX_CONTEXT_RETRY_FAILURES=0
typeset -gF _DMUX_CONTEXT_RETRY_FAILED_AT=0
typeset -gF _DMUX_CONTEXT_RETRY_AFTER=0
typeset -gi _DMUX_CONTEXT_TIMEOUT_SECONDS=5

_dmux_context_public_marker_present() {
  (( ${+DMUX_CONTEXT_VERSION}
    || ${+DMUX_HOST_UID}
    || ${+DMUX_SPACE_UID}
    || ${+DMUX_SPACE_NO}
    || ${+DMUX_BACKEND}
    || ${+DMUX_DOMAIN}
    || ${+DMUX_SERVER_EPOCH}
    || ${+DMUX_GROUP_REF}
    || ${+DMUX_SPLIT_REF} ))
}

_dmux_context_retry_due() {
  local now=${EPOCHREALTIME:-$SECONDS}
  (( _DMUX_CONTEXT_RETRY_AFTER <= 0 )) && return 0
  # A backward wall-clock jump must not extend a stale retry/cache window.
  (( now - _DMUX_CONTEXT_RETRY_FAILED_AT < 0 )) && return 0
  (( now >= _DMUX_CONTEXT_RETRY_AFTER ))
}

_dmux_context_schedule_retry() {
  local now=${EPOCHREALTIME:-$SECONDS} delay
  (( _DMUX_CONTEXT_RETRY_FAILURES++ ))
  case $_DMUX_CONTEXT_RETRY_FAILURES in
    1) delay=1 ;;
    2) delay=5 ;;
    *) delay=30 ;;
  esac
  _DMUX_CONTEXT_RETRY_FAILED_AT=$now
  _DMUX_CONTEXT_RETRY_AFTER=$(( now + delay ))
}

_dmux_context_reset_retry() {
  _DMUX_CONTEXT_RETRY_FAILURES=0
  _DMUX_CONTEXT_RETRY_FAILED_AT=0
  _DMUX_CONTEXT_RETRY_AFTER=0
}

_dmux_context_signature() {
  # Record both the native pane and every claimed marker. The record is used
  # only for a 200 ms duplicate-precmd cache; it is never executed or emitted.
  REPLY="${TMUX-}"$'\x1e'"${TMUX_PANE-}"$'\x1e'"${WEZTERM_PANE-}"$'\x1e'\
"${DMUX_CONTEXT_VERSION-}"$'\x1e'"${DMUX_HOST_UID-}"$'\x1e'\
"${DMUX_SPACE_UID-}"$'\x1e'"${DMUX_SPACE_NO-}"$'\x1e'\
"${DMUX_BACKEND-}"$'\x1e'"${DMUX_DOMAIN-}"$'\x1e'\
"${DMUX_SERVER_EPOCH-}"$'\x1e'"${DMUX_GROUP_REF-}"$'\x1e'\
"${DMUX_SPLIT_REF-}"
}

_dmux_context_tmux_client_owns_pane() {
  [[ -n ${TMUX:-} && -n ${TMUX_PANE:-} ]] || return 1
  (( $+commands[tmux] )) || return 1
  [[ -x /usr/bin/perl && -x /usr/bin/head ]] || return 1

  # `allow-passthrough all` forwards DCS from invisible panes. There is no
  # client-addressed passthrough form, so emit only when a COMPLETE client
  # inventory proves this server has exactly one client and that client's
  # exact active session/window/pane is the invoking pane. Zero, multiple,
  # malformed, racing, or mismatched rows all suppress output.
  local format output target client deadline
  local -a rows
  local -a fields
  format='#{session_id}'$'\x1f''#{window_id}'$'\x1f''#{pane_id}'
  deadline=$(( ${EPOCHREALTIME:-$SECONDS} + _DMUX_CONTEXT_TIMEOUT_SECONDS ))
  output="$(_dmux_context_run_tmux_bounded "$deadline" \
    display-message -p -t "$TMUX_PANE" "$format" ';' \
    list-clients -F "$format")" \
    || return 1
  (( $#output <= 8192 )) || return 1
  rows=("${(@f)output}")
  (( $#rows == 2 )) || return 1
  target=$rows[1]
  client=$rows[2]
  [[ -n $target && $client == "$target" ]] || return 1

  fields=("${(@ps:\x1f:)target}")
  (( $#fields == 3 )) || return 1
  [[ $fields[1] == '$'<->
    && $fields[2] == '@'<->
    && $fields[3] == '%'<->
    && $fields[3] == "$TMUX_PANE" ]]
}

_dmux_context_remaining_timeout() {
  local deadline=$1 now=${EPOCHREALTIME:-$SECONDS} remaining
  local -i timeout
  remaining=$(( deadline - now ))
  (( remaining > 0 )) || return 1
  timeout=$remaining
  (( timeout < remaining )) && (( timeout++ ))
  (( timeout >= 1 && timeout <= 30 )) || return 1
  REPLY=$timeout
}

_dmux_context_run_tmux_bounded() {
  emulate -L zsh
  setopt localoptions no_aliases pipe_fail

  local deadline=$1 REPLY
  shift
  _dmux_context_remaining_timeout "$deadline" || return 1
  local timeout=$REPLY
  LC_ALL=C.UTF-8 /usr/bin/perl -e 'alarm shift; exec @ARGV or exit 127' \
    "$timeout" "${commands[tmux]}" "$@" 2>/dev/null \
    | /usr/bin/head -c 8193
}

_dmux_context_emit() {
  local -a encoded=("$@")
  local -a names=(
    dmux_context_version
    dmux_host_uid
    dmux_space_uid
    dmux_space_no
    dmux_backend
    dmux_domain
    dmux_server_epoch
    dmux_group_ref
    dmux_split_ref
    dmux_tmux_client_uid
  )
  local index inner wrapped in_tmux=0

  (( $#encoded == 9 || $#encoded == 10 )) || return 1
  if [[ -n ${TMUX:-} ]]; then
    _dmux_context_tmux_client_owns_pane || return 0
    in_tmux=1
  fi
  for (( index = 1; index <= $#encoded; index++ )); do
    inner=$'\e]1337;SetUserVar='"${names[index]}=${encoded[index]}"$'\a'
    if (( in_tmux )); then
      # ADR 005's exact passthrough recipe: DCS `tmux;`, double every ESC in
      # the OSC payload, then terminate the wrapper with ST.
      wrapped=${inner//$'\e'/$'\e\e'}
      builtin printf '%s' $'\ePtmux;'"$wrapped"$'\e\\'
    else
      builtin printf '%s' "$inner"
    fi
  done
}

_dmux_context_clear() {
  builtin unset DMUX_CONTEXT_VERSION DMUX_HOST_UID DMUX_SPACE_UID DMUX_SPACE_NO
  builtin unset DMUX_BACKEND DMUX_DOMAIN DMUX_SERVER_EPOCH DMUX_GROUP_REF DMUX_SPLIT_REF
  _DMUX_CONTEXT_REFRESHED_AT=0
  _DMUX_CONTEXT_REFRESH_SIGNATURE=
  _DMUX_CONTEXT_REFRESH_B64=()

  # SetUserVar has no separate delete operation. Empty values make every
  # required field invalid to the GUI parser, which is the fail-closed state.
  _dmux_context_emit '' '' '' '' '' '' '' '' ''
}

_dmux_context_fail_closed() {
  local reason=$1
  _dmux_context_clear
  [[ -n $_DMUX_CONTEXT_SPACE_UID_HINT ]] && _dmux_context_schedule_retry
  builtin print -ru2 -- "dmux: $reason; pane markers cleared"
  return 0
}

_dmux_context_run_bounded() {
  emulate -L zsh
  setopt localoptions no_aliases pipe_fail

  local context_space_uid=$1 deadline=$2 REPLY
  _dmux_context_remaining_timeout "$deadline" || return 1
  local timeout=$REPLY

  # Both supported hosts provide these base-system paths. The alarm survives
  # exec and bounds controller/provider startup as well as the RPC itself;
  # head bounds command substitution before zsh can allocate an arbitrary
  # response. `pipe_fail` preserves a controller failure or timeout.
  local pipeline_status
  LC_ALL=C.UTF-8 DMUX_SPACE_UID="$context_space_uid" \
    /usr/bin/perl -e 'alarm shift; exec @ARGV or exit 127' \
      "$timeout" "${commands[dmux]}" _context 2>/dev/null \
    | /usr/bin/head -c 8193
  pipeline_status=$?
  # A non-newline sentinel prevents command substitution from silently
  # stripping trailing response bytes before the 8192-byte check.
  builtin printf '\x1e'
  return $pipeline_status
}

_dmux_context_parse_bounded() {
  emulate -L zsh
  setopt localoptions no_aliases

  local jq_filter=$1 context_space_uid=$2 deadline=$3 REPLY
  _dmux_context_remaining_timeout "$deadline" || return 1
  local timeout=$REPLY

  /usr/bin/perl -e 'alarm shift; exec @ARGV or exit 127' \
    "$timeout" "${commands[jq]}" -erj \
      --arg requested_space_uid "$context_space_uid" "$jq_filter" 2>/dev/null
}

_dmux_context_refresh_impl() {
  local public_marker=0
  _dmux_context_public_marker_present && public_marker=1

  if [[ -n ${DMUX_SPACE_UID:-} ]]; then
    if [[ -n $_DMUX_CONTEXT_SPACE_UID_HINT
      && $DMUX_SPACE_UID != $_DMUX_CONTEXT_SPACE_UID_HINT ]]; then
      _dmux_context_reset_retry
    fi
    _DMUX_CONTEXT_SPACE_UID_HINT=$DMUX_SPACE_UID
  fi
  (( public_marker )) || [[ -n $_DMUX_CONTEXT_SPACE_UID_HINT ]] || return 0
  (( public_marker )) || _dmux_context_retry_due || return 0

  local signature now=${EPOCHREALTIME:-$SECONDS} REPLY
  _dmux_context_signature
  signature=$REPLY

  # Starship/direnv/plugin stacks can cause duplicate prompt redraws. Re-emit
  # the last validated marker so an outer Wez prompt still restores itself
  # after nested tmux, but avoid another provider scan within the same burst.
  if (( now > 0 && _DMUX_CONTEXT_REFRESHED_AT > 0
        && now - _DMUX_CONTEXT_REFRESHED_AT >= 0
        && now - _DMUX_CONTEXT_REFRESHED_AT < 0.2
        && $#_DMUX_CONTEXT_REFRESH_B64 == 9 )) \
      && [[ $signature == $_DMUX_CONTEXT_REFRESH_SIGNATURE ]]; then
    if [[ $DMUX_BACKEND == wez ]]; then
      _dmux_context_emit "${_DMUX_CONTEXT_REFRESH_B64[@]}" ''
    else
      _dmux_context_emit "${_DMUX_CONTEXT_REFRESH_B64[@]}"
    fi
    return 0
  fi

  local context_space_uid=${DMUX_SPACE_UID:-$_DMUX_CONTEXT_SPACE_UID_HINT}
  [[ -n $context_space_uid ]] \
    || { _dmux_context_fail_closed 'pane context is incomplete'; return 0; }
  (( ${+TMUX_PANE} || ${+WEZTERM_PANE} )) \
    || { _dmux_context_fail_closed 'native pane identity is unavailable'; return 0; }
  (( $+commands[dmux] )) \
    || { _dmux_context_fail_closed 'dmux executable is unavailable'; return 0; }
  (( $+commands[jq] )) \
    || { _dmux_context_fail_closed 'jq is unavailable for context validation'; return 0; }
  [[ -x /usr/bin/perl && -x /usr/bin/head ]] \
    || { _dmux_context_fail_closed 'context deadline helpers are unavailable'; return 0; }

  local total_timeout=$_DMUX_CONTEXT_TIMEOUT_SECONDS
  (( total_timeout >= 1 && total_timeout <= 30 )) || total_timeout=5
  local deadline=$(( ${EPOCHREALTIME:-$SECONDS} + total_timeout ))
  local framed document parsed
  framed="$(_dmux_context_run_bounded "$context_space_uid" "$deadline")" \
    || { _dmux_context_fail_closed 'pane context is invalid or stale'; return 0; }
  [[ $framed == *$'\x1e' ]] \
    || { _dmux_context_fail_closed 'pane context response is malformed'; return 0; }
  document=${framed%$'\x1e'}
  (( $#document <= 8192 )) \
    || { _dmux_context_fail_closed 'pane context response is oversized'; return 0; }

  # Validate the complete JSON shape and all marker grammars inside jq, then
  # return both raw and base64 forms separated by a byte forbidden by those
  # grammars. No JSON-derived text is ever reparsed as shell syntax.
  local jq_filter='
    def uuid:
      type == "string"
      and test("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$");
    def domain:
      . == null
      or (type == "string" and test("^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$"));
    def child($kind; $epoch; $provider):
      type == "string"
      and test("^" + $kind + $epoch
        + "\\.(?:" + $provider + "-(?:0|[1-9][0-9]*)|x-[A-Za-z0-9_-]+)$");
    . as $c
    | if (
        type == "object"
        and keys == [
          "backend", "domain", "group_ref", "host_uid", "server_epoch",
          "space_no", "space_uid", "split_ref"
        ]
        and ($c.host_uid | uuid)
        and ($c.space_uid | uuid)
        and ($c.space_uid == $requested_space_uid)
        and ($c.server_epoch | uuid)
        and ($c.space_no | type == "number" and . >= 1
          and . <= 9007199254740991 and floor == .)
        and ($c.backend == "wez" or $c.backend == "tmux")
        and ($c.domain | domain)
        and ($c.group_ref | child("g"; $c.server_epoch;
          (if $c.backend == "wez" then "wz" else "tx" end)))
        and ($c.split_ref | child("p"; $c.server_epoch;
          (if $c.backend == "wez" then "wz" else "tx" end)))
      ) then
        [
          "1", $c.host_uid, $c.space_uid, ($c.space_no | tostring),
          $c.backend, ($c.domain // ""), $c.server_epoch,
          $c.group_ref, $c.split_ref
        ] as $raw
        | ($raw + ($raw | map(@base64)))
        | join("\u001f")
      else
        error("invalid dmux context")
      end
  '
  parsed="$(_dmux_context_parse_bounded \
    "$jq_filter" "$context_space_uid" "$deadline" <<< "$document")" \
    || { _dmux_context_fail_closed 'pane context response is malformed'; return 0; }

  local -a fields=("${(@ps:\x1f:)parsed}")
  (( $#fields == 18 )) \
    || { _dmux_context_fail_closed 'pane context response is malformed'; return 0; }

  builtin export DMUX_CONTEXT_VERSION=$fields[1]
  builtin export DMUX_HOST_UID=$fields[2]
  builtin export DMUX_SPACE_UID=$fields[3]
  builtin export DMUX_SPACE_NO=$fields[4]
  builtin export DMUX_BACKEND=$fields[5]
  builtin export DMUX_DOMAIN=$fields[6]
  builtin export DMUX_SERVER_EPOCH=$fields[7]
  builtin export DMUX_GROUP_REF=$fields[8]
  builtin export DMUX_SPLIT_REF=$fields[9]

  _DMUX_CONTEXT_REFRESH_B64=(
    "$fields[10]" "$fields[11]" "$fields[12]" "$fields[13]" "$fields[14]"
    "$fields[15]" "$fields[16]" "$fields[17]" "$fields[18]"
  )
  _dmux_context_signature
  _DMUX_CONTEXT_REFRESH_SIGNATURE=$REPLY
  _DMUX_CONTEXT_REFRESHED_AT=${EPOCHREALTIME:-$SECONDS}
  _DMUX_CONTEXT_SPACE_UID_HINT=$DMUX_SPACE_UID
  _dmux_context_reset_retry
  if [[ $DMUX_BACKEND == wez ]]; then
    # Leaving tmux returns to an outer Wez pane. A prior attach UID is not
    # valid authority there and must not poison later GUI-origin actions.
    _dmux_context_emit "${_DMUX_CONTEXT_REFRESH_B64[@]}" ''
  else
    _dmux_context_emit "${_DMUX_CONTEXT_REFRESH_B64[@]}"
  fi
}

_dmux_context_refresh() {
  emulate -L zsh
  setopt localoptions no_aliases
  (( _DMUX_CONTEXT_REFRESH_ACTIVE )) && return 0
  _DMUX_CONTEXT_REFRESH_ACTIVE=1
  {
    _dmux_context_refresh_impl
  } always {
    _DMUX_CONTEXT_REFRESH_ACTIVE=0
  }
  return 0
}

add-zsh-hook precmd _dmux_context_refresh

if [[ $_DMUX_CONTEXT_SOURCE_ALIASES == on ]]; then
  builtin unset _DMUX_CONTEXT_SOURCE_ALIASES
  builtin setopt aliases
else
  builtin unset _DMUX_CONTEXT_SOURCE_ALIASES
fi
