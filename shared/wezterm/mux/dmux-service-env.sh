#!/bin/sh
# dmux per-host service environment: the one parser for
# ~/.config/dmux/service.env (ADR 012 WS-F.1; plan §21 steps 7 and 9; the
# three-valued DMUX_WEZ_FIRST of ADR 010 §5).
#
# `launchctl setenv` and `systemctl --user set-environment` are runtime-only:
# a reboot clears them, and a mux that comes up without the flag is silently
# legacy. The durable source on macOS is the untracked, host-local file
# ~/.config/dmux/service.env. This helper is SOURCED (functions only, no side
# effects) by the two tracked readers of that file:
#   - shared/wezterm/mux/dmux-env-load.sh, the com.fredrir.dmux-env
#     LaunchAgent that copies each KEY=VALUE into the launchd gui session with
#     `launchctl setenv`, which is where the GUI reads the flag at launch;
#   - shared/wezterm/mux/dmux-mux-start.sh, which reads the file itself so
#     the managed mux never depends on LaunchAgent ordering.
# `dmux doctor` re-implements the same grammar in Rust to report the file
# layer; keep the three in step.
#
# The file is a privileged write into the GUI session environment, so the
# grammar is deliberately tiny and is checked with shell pattern matching
# only: never eval, never `.`/source, never a command built from file text.
#   - a blank line, or a line whose first non-blank character is `#`, is
#     ignored;
#   - every other line is KEY=VALUE: KEY matches ^DMUX_[A-Z0-9_]*$ and VALUE
#     matches ^[A-Za-z0-9_./:@+,-]*$ -- no whitespace, quotes, `$`,
#     backticks, `;`, `&`, `|`, `<`, `>`, braces, parentheses, `~`, `\` or
#     control characters, and no leading/trailing whitespace around either;
#   - a later assignment to the same KEY wins, as it would in a shell;
#   - ONE malformed line refuses the WHOLE file: each bad line is reported on
#     stderr by number and reason (never by content, which may be hostile)
#     and nothing is applied, so a typo can never half-apply a policy.
# An absent file is not an error: it means "no preference" at this layer.
#
# The character sets are spelled out rather than written as ranges: in a
# UTF-8 locale `[A-Z]` collates lowercase letters in on macOS's /bin/sh, and
# an explicit list matches the same bytes in every locale and every sh.
DMUX_SERVICE_ENV_KEY_CHARS='ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_'
DMUX_SERVICE_ENV_VALUE_CHARS='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./:@+,-'

# Where the file lives. Honours XDG_CONFIG_HOME (the test sandbox sets it;
# launchd and systemd do not). Prints nothing and fails when neither
# XDG_CONFIG_HOME nor HOME is set, so a caller under `set -u` gets a status
# rather than an unbound-variable abort.
dmux_service_env_path() {
  if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    printf '%s/dmux/service.env\n' "${XDG_CONFIG_HOME%/}"
  elif [ -n "${HOME:-}" ]; then
    printf '%s/.config/dmux/service.env\n' "${HOME%/}"
  else
    return 1
  fi
}

# dmux_service_env_lines FILE
#   stdout: the validated KEY=VALUE lines of FILE, in file order, trimmed of
#           leading whitespace; nothing when FILE is absent or has no
#           assignments.
#   status: 0 on success (including absent/empty);
#           2 when FILE is malformed -- nothing is printed, and every
#             offending line is reported on stderr as FILE:LINE: reason.
dmux_service_env_lines() {
  _dse_file=$1
  _dse_out=''
  _dse_bad=0
  _dse_n=0
  [ -e "$_dse_file" ] || return 0
  if [ ! -f "$_dse_file" ] || [ ! -r "$_dse_file" ]; then
    echo "dmux-service-env: $_dse_file: not a readable regular file" >&2
    return 2
  fi
  while IFS= read -r _dse_line || [ -n "$_dse_line" ]; do
    _dse_n=$((_dse_n + 1))
    # Strip leading whitespace; ${line%%[![:space:]]*} is the leading run.
    _dse_trim=${_dse_line#"${_dse_line%%[![:space:]]*}"}
    case "$_dse_trim" in
    '' | \#*) continue ;;
    esac
    _dse_why=''
    case "$_dse_trim" in
    *=*) ;;
    *) _dse_why='expected KEY=VALUE' ;;
    esac
    if [ -z "$_dse_why" ]; then
      _dse_key=${_dse_trim%%=*}
      _dse_value=${_dse_trim#*=}
      case "$_dse_key" in
      DMUX_*)
        case "${_dse_key#DMUX_}" in
        *[!$DMUX_SERVICE_ENV_KEY_CHARS]*) _dse_why='key must match ^DMUX_[A-Z0-9_]*$' ;;
        esac
        ;;
      *) _dse_why='key must start with DMUX_' ;;
      esac
    fi
    if [ -z "$_dse_why" ]; then
      case "$_dse_value" in
      *[!$DMUX_SERVICE_ENV_VALUE_CHARS]*)
        _dse_why='value must match ^[A-Za-z0-9_./:@+,-]*$ (no whitespace, quotes, $, backticks or ;)'
        ;;
      esac
    fi
    if [ -n "$_dse_why" ]; then
      echo "dmux-service-env: $_dse_file:$_dse_n: $_dse_why" >&2
      _dse_bad=1
      continue
    fi
    _dse_out="$_dse_out$_dse_trim
"
  done <"$_dse_file"
  if [ "$_dse_bad" -ne 0 ]; then
    echo "dmux-service-env: $_dse_file: refused; nothing applied" >&2
    return 2
  fi
  printf '%s' "$_dse_out"
}

# dmux_service_env_lookup KEY LINES
#   LINES is the output of dmux_service_env_lines. Prints the value of the
#   LAST assignment to KEY; status 1 when KEY is not assigned. Pure string
#   operations on already-validated text.
dmux_service_env_lookup() {
  _dse_want=$1
  _dse_rest=$2
  _dse_found=1
  _dse_result=''
  _dse_nl='
'
  while [ -n "$_dse_rest" ]; do
    case "$_dse_rest" in
    *"$_dse_nl"*)
      _dse_line=${_dse_rest%%"$_dse_nl"*}
      _dse_rest=${_dse_rest#*"$_dse_nl"}
      ;;
    *)
      _dse_line=$_dse_rest
      _dse_rest=''
      ;;
    esac
    case "$_dse_line" in
    "$_dse_want="*)
      _dse_found=0
      _dse_result=${_dse_line#*=}
      ;;
    esac
  done
  [ "$_dse_found" -eq 0 ] && printf '%s\n' "$_dse_result"
  return "$_dse_found"
}
