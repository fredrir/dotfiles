#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

DOTFILES="$(cd "$(dirname "$0")" && pwd)"
STATE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/dotfile"
TOOL_BIN_DIR="$HOME/.local/bin"
TOOL_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/uv/tools"
DOTFILE_BIN="$TOOL_BIN_DIR/dotfile"
DOTFILE_BACKEND_BIN="$TOOL_BIN_DIR/dotfile-py"
COMMANDS_ONLY=0
SYNC=0
ARG_PROFILE=""
LINK_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
  --commands-only) COMMANDS_ONLY=1 ;;
  --sync) SYNC=1 ;;
  --)
    shift
    LINK_ARGS=("$@")
    break
    ;;
  --?*) ARG_PROFILE="${1#--}" ;;
  ?*) ARG_PROFILE="$1" ;;
  esac
  shift
done

STAMP_DIR="$STATE_DIR/sync"
SETUP_LOCK_DIR="$STATE_DIR/setup.lock.d"
SETUP_LOCK_KIND=""
SETUP_STAGE=""
SETUP_NATIVE_TRANSACTION=0
SETUP_DEFERRED_SIGNAL=0

settled() (
  trap '' HUP INT TERM
  "$@"
)

record_setup_signal() {
  case "$1" in
  HUP) SETUP_DEFERRED_SIGNAL=1 ;;
  INT) SETUP_DEFERRED_SIGNAL=2 ;;
  TERM) SETUP_DEFERRED_SIGNAL=15 ;;
  esac
}

finish_deferred_signal() {
  local deferred="$SETUP_DEFERRED_SIGNAL"
  trap - HUP INT TERM
  SETUP_DEFERRED_SIGNAL=0
  if [ "$deferred" -gt 0 ]; then
    exit $((128 + deferred))
  fi
}

rollback_native_install() {
  [ "$SETUP_NATIVE_TRANSACTION" = 1 ] || return 0
  local name failed=0
  case "$SETUP_STAGE" in
  "$TOOL_BIN_DIR"/.dotfile-native.*)
    for name in $RUST_BINARIES; do
      [ "$name" = dotfile ] && continue
      if [ -e "$SETUP_STAGE/.previous/$name" ] || [ -L "$SETUP_STAGE/.previous/$name" ]; then
        if ! settled mv -f "$SETUP_STAGE/.previous/$name" "$TOOL_BIN_DIR/$name"; then
          echo "setup: could not restore native tool '$name'" >&2
          failed=1
        fi
      elif [ -f "$SETUP_STAGE/.absent/$name" ]; then
        if ! settled rm -f -- "$TOOL_BIN_DIR/$name"; then
          echo "setup: could not remove partially installed native tool '$name'" >&2
          failed=1
        fi
      fi
    done
    name=dotfile
    if [ -e "$SETUP_STAGE/.previous/$name" ] || [ -L "$SETUP_STAGE/.previous/$name" ]; then
      if ! settled mv -f "$SETUP_STAGE/.previous/$name" "$TOOL_BIN_DIR/$name"; then
        echo "setup: could not restore native tool '$name'" >&2
        failed=1
      fi
    elif [ -f "$SETUP_STAGE/.absent/$name" ]; then
      if ! settled rm -f -- "$TOOL_BIN_DIR/$name"; then
        echo "setup: could not remove partially installed native tool '$name'" >&2
        failed=1
      fi
    fi
    ;;
  esac
  if [ "$failed" = 0 ]; then
    SETUP_NATIVE_TRANSACTION=0
  fi
  return "$failed"
}

release_setup_lock() {
  if [ "$SETUP_LOCK_KIND" = mkdir ]; then
    local owner
    owner="$(cat "$SETUP_LOCK_DIR/pid" 2>/dev/null || true)"
    if [ -z "$owner" ] || [ "$owner" = "$$" ]; then
      rm -f -- "$SETUP_LOCK_DIR/pid"
      rmdir "$SETUP_LOCK_DIR" 2>/dev/null || true
    fi
  fi
  SETUP_LOCK_KIND=""
}

cleanup_setup() {
  local status=$?
  if rollback_native_install; then
    case "$SETUP_STAGE" in
    "$TOOL_BIN_DIR"/.dotfile-native.*) settled rm -rf -- "$SETUP_STAGE" ;;
    esac
  else
    status=1
    echo "setup: rollback incomplete; recovery files remain at $SETUP_STAGE" >&2
  fi
  release_setup_lock
  if [ -t 1 ]; then
    printf '\033[?25h'
  fi
  return "$status"
}

acquire_setup_lock() {
  mkdir -p "$STATE_DIR"
  local owner stale announced=0 missing=0
  while ! mkdir "$SETUP_LOCK_DIR" 2>/dev/null; do
    owner="$(cat "$SETUP_LOCK_DIR/pid" 2>/dev/null || true)"
    if [[ "$owner" =~ ^[0-9]+$ ]] && ! kill -0 "$owner" 2>/dev/null; then
      stale="$SETUP_LOCK_DIR.stale.$$"
      if mv "$SETUP_LOCK_DIR" "$stale" 2>/dev/null; then
        rm -rf -- "$stale"
      fi
      continue
    fi
    if [ "$announced" = 0 ]; then
      echo "another setup is running; waiting"
      announced=1
    fi
    if [ -z "$owner" ]; then
      missing=$((missing + 1))
      if [ "$missing" -ge 25 ]; then
        echo "setup: lock has no owner; remove $SETUP_LOCK_DIR if no setup is running" >&2
        exit 1
      fi
    else
      missing=0
    fi
    sleep 0.2
  done
  SETUP_LOCK_KIND="mkdir"
  if ! printf '%s\n' "$$" >"$SETUP_LOCK_DIR/pid"; then
    release_setup_lock
    exit 1
  fi
}

trap cleanup_setup EXIT

if command -v sha256sum >/dev/null 2>&1; then
  HASHER="sha256sum"
else
  HASHER="shasum -a 256"
fi

content_hash() { cat "$@" 2>/dev/null | $HASHER | cut -d' ' -f1; }

unchanged() { [ "$(cat "$STAMP_DIR/$1" 2>/dev/null)" = "$2" ]; }

stamp() {
  mkdir -p "$STAMP_DIR"
  local temporary
  temporary="$(mktemp "$STAMP_DIR/.$1.XXXXXX")"
  printf '%s\n' "$2" >"$temporary"
  mv -f "$temporary" "$STAMP_DIR/$1"
}

BOLD=$'\033[1m'
DIM=$'\033[2m'
CYAN=$'\033[36m'
RESET=$'\033[0m'
PICKED=""

interactive() { [ -t 0 ] && [ -t 1 ]; }

saved_profile() {
  if [ -f "$STATE_DIR/profile" ]; then
    cat "$STATE_DIR/profile"
  fi
}

saved_override() {
  [ -f "$STATE_DIR/overrides" ] || return 0
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
    "$1="*)
      printf '%s\n' "${line#*=}"
      return 0
      ;;
    esac
  done <"$STATE_DIR/overrides"
}

pick() {
  local title="$1" default="$2"
  shift 2
  local opts=("$@") count=$# idx=0 first=1 i key rest
  for i in "${!opts[@]}"; do
    [ "${opts[$i]}" = "$default" ] && idx="$i"
  done
  printf '\n  %s%s%s\n' "$BOLD" "$title" "$RESET"
  printf '  %s↑/↓ move | ↩ select | q quit%s\n\n' "$DIM" "$RESET"
  printf '\033[?25l'
  while :; do
    if [ "$first" = 0 ]; then
      printf '\033[%dA' "$count"
    fi
    first=0
    for i in "${!opts[@]}"; do
      if [ "$i" -eq "$idx" ]; then
        printf '  %s%s❯ %s%s\033[K\n' "$CYAN" "$BOLD" "${opts[$i]}" "$RESET"
      else
        printf '    %s\033[K\n' "${opts[$i]}"
      fi
    done
    IFS= read -rsn1 key </dev/tty || key=""
    case "$key" in
    $'\033')
      IFS= read -rsn2 -t 1 rest </dev/tty || rest=""
      case "$rest" in
      '[A') idx=$(((idx + count - 1) % count)) ;;
      '[B') idx=$(((idx + 1) % count)) ;;
      esac
      ;;
    k) idx=$(((idx + count - 1) % count)) ;;
    j) idx=$(((idx + 1) % count)) ;;
    [1-9])
      if [ "$key" -le "$count" ]; then
        idx=$((key - 1))
      fi
      ;;
    '' | $'\n' | $'\r') break ;;
    q)
      printf '\033[?25h\n'
      exit 130
      ;;
    esac
  done
  printf '\033[?25h'
  PICKED="${opts[$idx]}"
}

git -C "$DOTFILES" config core.hooksPath "$DOTFILES/.githooks" 2>/dev/null || true

AGE_KEY_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/dotfile/age/keys.txt"
git -C "$DOTFILES" config diff.sops.textconv \
  "SOPS_AGE_KEY_FILE=$AGE_KEY_FILE sops -d" 2>/dev/null || true
git -C "$DOTFILES" config diff.sops.cachetextconv false 2>/dev/null || true

if ! command -v uv >/dev/null 2>&1; then
  echo "setup: uv is required (https://docs.astral.sh/uv/) to install the workstation tools" >&2
  exit 1
fi

acquire_setup_lock

PYTHON_HASH="$(content_hash "$DOTFILES/scripts/python/pyproject.toml" "$DOTFILES/scripts/python/uv.lock")"

python_current() {
  [ -x "$DOTFILE_BACKEND_BIN" ] || return 1
  if [ "$COMMANDS_ONLY" = 0 ] && [ ! -x "$DOTFILES/scripts/python/.venv/bin/dotfile-py" ]; then
    return 1
  fi
  unchanged python "$PYTHON_HASH"
}

if python_current; then
  echo "workstation commands are current"
else
  if [ "$COMMANDS_ONLY" = 0 ]; then
    echo "syncing workstation tools (scripts/python/.venv)"
    uv sync --project "$DOTFILES/scripts/python" --locked --compile-bytecode --quiet
  fi
  echo "installing workstation commands (~/.local/bin)"
  mkdir -p "$TOOL_BIN_DIR"
  UV_TOOL_BIN_DIR="$TOOL_BIN_DIR" UV_TOOL_DIR="$TOOL_DIR" \
    uv tool install \
    --compile-bytecode \
    --constraints <(
      uv export --project "$DOTFILES/scripts/python" --locked --no-dev --no-emit-project \
        --no-header --no-annotate --no-hashes --quiet
    ) \
    --editable --reinstall --quiet "$DOTFILES/scripts/python"
  stamp python "$PYTHON_HASH"
fi

RUST_BINARIES="agent-hop bench-workloads count doc-keybinds doc-purge dotfile dotfile-format dotfmt flatten gget git-discard gppf hpull hpush hwire mux-route path size sysinfo-collect tmux-workspace"
RUST_HASH="$(
  find "$DOTFILES/scripts/rust" "$DOTFILES/shared/tools" \
    -type d -name target -prune -o -type f -print0 2>/dev/null |
    sort -z | xargs -0 cat 2>/dev/null | $HASHER | cut -d' ' -f1
)"

rust_current() {
  local name
  for name in $RUST_BINARIES; do
    [ -x "$TOOL_BIN_DIR/$name" ] || return 1
  done
  "$DOTFILE_BIN" sync --version >/dev/null 2>&1 || return 1
  unchanged rust "$RUST_HASH"
}

if ! command -v cargo >/dev/null 2>&1; then
  echo "setup: cargo is required to build dotfile" >&2
  exit 1
elif rust_current; then
  echo "native tools are current"
else
  echo "building native tools (scripts/rust)"
  if cargo build --release --locked --quiet --manifest-path "$DOTFILES/scripts/rust/Cargo.toml"; then
    SETUP_STAGE="$(mktemp -d "$TOOL_BIN_DIR/.dotfile-native.XXXXXX")"
    for name in $RUST_BINARIES; do
      install -m 0755 "$DOTFILES/scripts/rust/target/release/$name" "$SETUP_STAGE/$name"
    done
    "$SETUP_STAGE/dotfile" sync --version >/dev/null
    "$SETUP_STAGE/sysinfo-collect" --version >/dev/null
    mkdir "$SETUP_STAGE/.previous" "$SETUP_STAGE/.absent"
    for name in $RUST_BINARIES; do
      if [ -e "$TOOL_BIN_DIR/$name" ] || [ -L "$TOOL_BIN_DIR/$name" ]; then
        cp -pP "$TOOL_BIN_DIR/$name" "$SETUP_STAGE/.previous/$name"
      else
        : >"$SETUP_STAGE/.absent/$name"
      fi
    done
    trap 'record_setup_signal HUP' HUP
    trap 'record_setup_signal INT' INT
    trap 'record_setup_signal TERM' TERM
    SETUP_NATIVE_TRANSACTION=1
    for name in $RUST_BINARIES; do
      [ "$name" = dotfile ] && continue
      if ! settled mv -f "$SETUP_STAGE/$name" "$TOOL_BIN_DIR/$name"; then
        echo "setup: could not install native tool '$name'" >&2
        rollback_native_install || true
        finish_deferred_signal
        exit 1
      fi
    done
    if ! settled mv -f "$SETUP_STAGE/dotfile" "$DOTFILE_BIN"; then
      echo "setup: could not install dotfile" >&2
      rollback_native_install || true
      finish_deferred_signal
      exit 1
    fi
    if ! settled stamp rust "$RUST_HASH"; then
      echo "setup: could not record native tool installation" >&2
      rollback_native_install || true
      finish_deferred_signal
      exit 1
    fi
    SETUP_NATIVE_TRANSACTION=0
    settled rm -rf -- "$SETUP_STAGE"
    SETUP_STAGE=""
    finish_deferred_signal
  else
    echo "setup: cargo build failed" >&2
    exit 1
  fi
fi

if ! "$DOTFILE_BIN" completions --dir "$HOME/.cache/zsh" >/dev/null 2>&1; then
  echo "setup: could not write shell completions (continuing)" >&2
fi

release_setup_lock

if [ -x "$TOOL_BIN_DIR/git-discard" ] && { [ -f "$TOOL_BIN_DIR/gdd" ] || [ -L "$TOOL_BIN_DIR/gdd" ]; }; then
  rm -f -- "$TOOL_BIN_DIR/gdd"
fi

if [ "$COMMANDS_ONLY" = 1 ]; then
  exit 0
fi

if [ "$SYNC" = 1 ]; then
  sync_args=(sync)
  if [ -n "$ARG_PROFILE" ]; then
    sync_args+=("$ARG_PROFILE")
  fi
  sync_args+=("${LINK_ARGS[@]}")
  exec "$DOTFILE_BIN" "${sync_args[@]}"
fi

PROFILE="$ARG_PROFILE"

list_profiles() { "$DOTFILE_BIN" profiles; }

if [ -z "$PROFILE" ]; then
  if interactive; then
    profiles=()
    while IFS= read -r p; do
      profiles+=("$p")
    done < <("$DOTFILE_BIN" profiles --relevant)
    if [ "${#profiles[@]}" -eq 0 ]; then
      echo "setup: no relevant installed environment found" >&2
      echo "override detection with ./setup.sh --<environment>" >&2
      echo "available environments:" >&2
      list_profiles | sed 's/^/  --/' >&2
      exit 1
    fi
    default="$(saved_profile)"
    if [ -z "$default" ]; then
      case "$(uname -s)" in
      Darwin) default="macos" ;;
      esac
    fi
    pick "select environment" "$default" "${profiles[@]}"
    PROFILE="$PICKED"
  else
    PROFILE="$(saved_profile)"
    if [ -z "$PROFILE" ]; then
      echo "usage: ./setup.sh [--<environment>]" >&2
      echo "available profiles:" >&2
      list_profiles | sed 's/^/  /' >&2
      exit 1
    fi
  fi
fi

MANIFEST="$DOTFILES/environment/$PROFILE/manifest"
if [ ! -f "$MANIFEST" ]; then
  echo "setup: no manifest for profile '$PROFILE'" >&2
  echo "available profiles:" >&2
  list_profiles | sed 's/^/  /' >&2
  exit 1
fi

OVERRIDE_ARGS=()
if interactive; then
  while IFS= read -r group; do
    group="${group%%#*}"
    group="${group#"${group%%[![:space:]]*}"}"
    group="${group%"${group##*[![:space:]]}"}"
    [ -n "$group" ] || continue
    [ -d "$DOTFILES/$group/overrides" ] || continue
    names=()
    for d in "$DOTFILES/$group/overrides"/*/; do
      names+=("$(basename "${d%/}")")
    done
    [ "${#names[@]}" -gt 0 ] || continue
    pick "select machine override for $group" "$(saved_override "$group")" "${names[@]}" none
    OVERRIDE_ARGS+=(--override "$group=$PICKED")
  done <"$MANIFEST"
fi

if [ ! -f "$AGE_KEY_FILE" ]; then
  echo
  "$DOTFILE_BIN" secret init || true
fi

echo
sync_args=(sync "$PROFILE")
sync_args+=("${OVERRIDE_ARGS[@]}")
sync_args+=("${LINK_ARGS[@]}")
exec "$DOTFILE_BIN" "${sync_args[@]}"
