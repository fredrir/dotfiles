#!/usr/bin/env bash
set -euo pipefail

DOTFILES_REPO="${DOTFILES_REPO:-https://github.com/fredrir/dotfiles}"
DOTFILES_DIR="${DOTFILES_DIR:-$HOME/dotfiles}"
PROFILE="ubuntu/server"
USER_NAME="$(id -un)"

# Per-user tool dirs must be visible to this script *and* the resulting shell.
export PATH="$HOME/.local/bin:$HOME/.local/nvim/bin:$HOME/.cargo/bin:$PATH"

have() { command -v "$1" >/dev/null 2>&1; }
say()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m !! \033[0m%s\n' "$*" >&2; }

# --- privilege -------------------------------------------------------------
SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  if have sudo; then SUDO="sudo"; else
    warn "not root and no sudo — package installs will be skipped."
  fi
fi

# --- download helpers ------------------------------------------------------
fetch()        { # url dest
  if have curl; then curl -fsSL "$1" -o "$2"; elif have wget; then wget -qO "$2" "$1"; else return 1; fi
}
fetch_stdout() { # url
  if have curl; then curl -fsSL "$1"; elif have wget; then wget -qO- "$1"; else return 1; fi
}

# --- package manager -------------------------------------------------------
PM=""
for c in apt-get dnf pacman apk zypper; do have "$c" && { PM="$c"; break; }; done

pm_refresh() {
  case "$PM" in
    apt-get) $SUDO apt-get update -y ;;
    pacman)  $SUDO pacman -Sy --noconfirm ;;
    apk)     $SUDO apk update ;;
    zypper)  $SUDO zypper --non-interactive refresh ;;
  esac
}
pm_install() {
  case "$PM" in
    apt-get) $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y "$@" ;;
    dnf)     $SUDO dnf install -y "$@" ;;
    pacman)  $SUDO pacman -S --needed --noconfirm "$@" ;;
    apk)     $SUDO apk add "$@" ;;
    zypper)  $SUDO zypper --non-interactive install "$@" ;;
  esac
}

install_packages() {
  if [ -z "$PM" ]; then warn "no supported package manager found — skipping installs."; return; fi
  if [ -z "$SUDO" ] && [ "$(id -u)" -ne 0 ]; then return; fi

  say "Installing packages via $PM"
  pm_refresh || true

  local core=(zsh git curl ca-certificates)
  [ "$PM" = apk ] && core+=(bash)
  pm_install "${core[@]}"

  local fd=fd; case "$PM" in apt-get|dnf) fd=fd-find ;; esac
  local extras=(neovim fzf ripgrep "$fd" bat eza zoxide fastfetch jq \
                zsh-autosuggestions zsh-syntax-highlighting less tar unzip)

  if ! pm_install "${extras[@]}" >/dev/null 2>&1; then
    local p
    for p in "${extras[@]}"; do
      pm_install "$p" >/dev/null 2>&1 || warn "(skip) $p not available in $PM"
    done
  fi

  # The vps/linux nvim is minimal (no mason/LSP) but keeps treesitter, which
  # compiles its parsers from C on first launch — so it needs a compiler.
  local build
  case "$PM" in
    apt-get) build=(build-essential) ;;
    apk)     build=(build-base) ;;
    *)       build=(gcc make) ;;
  esac
  pm_install "${build[@]}" >/dev/null 2>&1 \
    || warn "(skip) C toolchain unavailable — treesitter parsers won't compile"
}

# --- modern neovim ---------------------------------------------------------
nvim_recent() {
  have nvim || return 1
  local v; v="$(nvim --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+' | head -1)"
  [ -n "$v" ] || return 1
  awk -v v="$v" 'BEGIN{split(v,a,"."); exit !(a[1]>0 || a[2]>=10)}'
}
install_nvim_tarball() {
  local arch tmp dir asset ok=""
  case "$(uname -m)" in
    x86_64|amd64)  arch=x86_64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) warn "no prebuilt nvim for $(uname -m); keeping packaged version."; return 0 ;;
  esac
  tmp="$(mktemp -d)"
  # Asset name changed across releases; try the current one then the legacy one.
  for asset in "nvim-linux-${arch}.tar.gz" "nvim-linux64.tar.gz"; do
    if fetch "https://github.com/neovim/neovim/releases/download/stable/${asset}" "$tmp/nvim.tar.gz"; then
      ok=1; break
    fi
  done
  [ -n "$ok" ] || { warn "couldn't download neovim tarball; keeping packaged version."; rm -rf "$tmp"; return 0; }
  tar -xzf "$tmp/nvim.tar.gz" -C "$tmp"
  dir="$(find "$tmp" -maxdepth 1 -type d -name 'nvim-*' | head -1)"
  mkdir -p "$HOME/.local"
  rm -rf "$HOME/.local/nvim"
  mv "$dir" "$HOME/.local/nvim"
  mkdir -p "$HOME/.local/bin"
  ln -sf "$HOME/.local/nvim/bin/nvim" "$HOME/.local/bin/nvim"
  rm -rf "$tmp"
  hash -r
  say "Installed $("$HOME/.local/nvim/bin/nvim" --version | head -1) to ~/.local/nvim"
}

# --- oh-my-zsh + starship --------------------------------------------------
install_ohmyzsh() {
  [ -d "$HOME/.oh-my-zsh" ] && return 0
  say "Installing oh-my-zsh"
  # KEEP_ZSHRC so it never clobbers the linked ~/.zshrc; CHSH/RUNZSH off — we do those.
  RUNZSH=no KEEP_ZSHRC=yes CHSH=no \
    sh -c "$(fetch_stdout https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)" "" --unattended \
    || warn "oh-my-zsh install failed; the prompt will fall back to a basic theme."
}
install_starship() {
  have starship && return 0
  say "Installing starship prompt"
  fetch_stdout https://starship.rs/install.sh | sh -s -- --yes --bin-dir "$HOME/.local/bin" \
    || warn "starship install failed; zsh will use the fallback prompt."
}

install_uv() {
  have uv && return 0
  say "Installing uv"
  if ! fetch_stdout https://astral.sh/uv/install.sh | env UV_NO_MODIFY_PATH=1 sh; then
    warn "uv install failed; dotfile commands will be unavailable."
  fi
  hash -r
}

# --- get the repo ----------------------------------------------------------
ensure_repo() {
  local self="${BASH_SOURCE[0]:-}"
  # Running from inside a clone? Use it.
  if [ -n "$self" ] && [ -f "$(dirname "$self")/setup.sh" ]; then
    DOTFILES_DIR="$(cd "$(dirname "$self")" && pwd)"
    return
  fi
  if [ -d "$DOTFILES_DIR/.git" ]; then
    say "Updating $DOTFILES_DIR"; git -C "$DOTFILES_DIR" pull --ff-only || warn "pull failed; using existing checkout."
  else
    say "Cloning $DOTFILES_REPO -> $DOTFILES_DIR"; git clone "$DOTFILES_REPO" "$DOTFILES_DIR"
  fi
}

install_dotfile_commands() {
  if ! have uv; then
    warn "uv unavailable; skipping dotfile commands."
    return 0
  fi
  say "Installing dotfile commands"
  "$DOTFILES_DIR/setup.sh" --commands-only \
    || warn "dotfile command installation failed."
}

# --- link dotfiles -----------------------------------------------------------
# Standalone per-file linker for the server profile: no dependency on the
# workstation Python tooling. Every tracked file gets its own symlink at the
# destination mapped by the targets file (default ~/.config/<package>/...).
declare -A LINK_TARGETS=()
LINK_CONFLICTS=0

load_link_targets() {
  LINK_TARGETS=()
  [ -f "$DOTFILES_DIR/targets" ] || return 0
  local line key value
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in *=*) ;; *) continue ;; esac
    key="${line%%=*}"; key="${key#"${key%%[![:space:]]*}"}"; key="${key%"${key##*[![:space:]]}"}"
    value="${line#*=}"; value="${value#"${value%%[![:space:]]*}"}"; value="${value%"${value##*[![:space:]]}"}"
    LINK_TARGETS["$key"]="${value/#\~/$HOME}"
  done < "$DOTFILES_DIR/targets"
}

map_link_dst() { # repo-relative path, package name, path inside package
  local full="$1" pkg="$2" rel="$3" key best=""
  for key in "${!LINK_TARGETS[@]}"; do
    if [ "$full" = "$key" ] || [ "${full#"$key"/}" != "$full" ]; then
      if [ "${#key}" -gt "${#best}" ]; then best="$key"; fi
    fi
  done
  if [ -z "$best" ]; then
    printf '%s\n' "$HOME/.config/$pkg${rel:+/$rel}"
  elif [ "$full" = "$best" ]; then
    printf '%s\n' "${LINK_TARGETS[$best]}"
  else
    printf '%s\n' "${LINK_TARGETS[$best]}/${full#"$best"/}"
  fi
}

dismantle_folded_ancestors() { # destination path
  local rest="${1#"$HOME"/}" current="$HOME" segment target
  while [ "$rest" != "${rest#*/}" ]; do
    segment="${rest%%/*}"
    rest="${rest#*/}"
    current="$current/$segment"
    [ -L "$current" ] || continue
    target="$(readlink "$current")"
    case "$target" in
      "$DOTFILES_DIR"/*)
        [ -d "$target" ] || return 0
        rm "$current"
        mkdir -p "$current"
        link_file_tree "$target" "$current"
        ;;
      *) return 0 ;;
    esac
  done
}

link_one_file() { # source file, destination path
  local src="$1" dst="$2" current
  dismantle_folded_ancestors "$dst"
  if [ -L "$dst" ]; then
    current="$(readlink "$dst")"
    if [ "$current" = "$src" ]; then return 0; fi
    case "$current" in
      "$DOTFILES_DIR"/*) rm "$dst" ;;
      *)
        warn "conflict: $dst is a foreign symlink — move it aside and re-run"
        LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
        return 0
        ;;
    esac
  elif [ -e "$dst" ]; then
    warn "conflict: $dst exists — move it aside and re-run"
    LINK_CONFLICTS=$((LINK_CONFLICTS + 1))
    return 0
  fi
  mkdir -p "$(dirname "$dst")"
  ln -s "$src" "$dst"
}

link_file_tree() { # source directory, destination directory
  local srcdir="$1" dstdir="$2" entry name
  shopt -s dotglob nullglob
  for entry in "$srcdir"/*; do
    name="$(basename "$entry")"
    [ "$name" = ".nolink" ] && continue
    if [ -d "$entry" ] && [ ! -L "$entry" ]; then
      link_file_tree "$entry" "$dstdir/$name"
    else
      link_one_file "$entry" "$dstdir/$name"
    fi
  done
}

link_package_files() { # group, package directory
  local group="$1" pkgdir="$2" pkg entry name rel full dst
  pkg="$(basename "$pkgdir")"
  while IFS= read -r entry; do
    name="$(basename "$entry")"
    [ "$name" = ".nolink" ] && continue
    rel="${entry#"$pkgdir"/}"
    full="$group/$pkg/$rel"
    dst="$(map_link_dst "$full" "$pkg" "$rel")"
    link_one_file "$entry" "$dst"
  done < <(find "$pkgdir" \( -type f -o -type l \) ! -name .nolink | LC_ALL=C sort)
}

prune_dead_repo_links() {
  local link
  while IFS= read -r link; do
    case "$(readlink "$link")" in
      "$DOTFILES_DIR"/*) [ -e "$link" ] || rm "$link" ;;
    esac
  done < <(
    find "$HOME" -maxdepth 1 -type l 2>/dev/null
    find "$HOME/.config" "$HOME/.local" -maxdepth 6 -type l 2>/dev/null
  )
}

link_dotfiles() {
  # A distro-provided ~/.zshrc would conflict with the linker; move it aside once.
  if [ -e "$HOME/.zshrc" ] && [ ! -L "$HOME/.zshrc" ]; then
    warn "backing up existing ~/.zshrc -> ~/.zshrc.pre-dotfiles"
    mv "$HOME/.zshrc" "$HOME/.zshrc.pre-dotfiles"
  fi
  say "Linking profile '$PROFILE'"

  local manifest="$DOTFILES_DIR/environment/$PROFILE/manifest" group pkgdir
  if [ ! -f "$manifest" ]; then
    warn "no manifest for profile '$PROFILE' — skipping dotfile links."
    SETUP_RC=1
    return
  fi

  load_link_targets
  prune_dead_repo_links

  while IFS= read -r group || [ -n "$group" ]; do
    group="${group%%#*}"
    group="${group#"${group%%[![:space:]]*}"}"
    group="${group%"${group##*[![:space:]]}"}"
    [ -n "$group" ] || continue
    if [ ! -d "$DOTFILES_DIR/$group" ]; then
      warn "skip missing group: $group"
      continue
    fi
    for pkgdir in "$DOTFILES_DIR/$group"/*/; do
      pkgdir="${pkgdir%/}"
      [ "$(basename "$pkgdir")" = "overrides" ] && continue
      [ -e "$pkgdir/.nolink" ] && continue
      link_package_files "$group" "$pkgdir"
    done
  done < "$manifest"

  SETUP_RC="$LINK_CONFLICTS"
}

# --- login shell -----------------------------------------------------------
set_login_shell() {
  [ -n "${NO_CHSH:-}" ] && return 0
  local zsh_path; zsh_path="$(command -v zsh)" || { warn "zsh not on PATH; skipping chsh."; return 0; }
  local cur; cur="$(getent passwd "$USER_NAME" 2>/dev/null | cut -d: -f7)"
  [ -z "$cur" ] && cur="$(grep "^$USER_NAME:" /etc/passwd 2>/dev/null | cut -d: -f7)"
  [ "$cur" = "$zsh_path" ] && return 0
  grep -qx "$zsh_path" /etc/shells 2>/dev/null || echo "$zsh_path" | $SUDO tee -a /etc/shells >/dev/null 2>&1 || true
  say "Setting login shell to $zsh_path"
  chsh -s "$zsh_path" 2>/dev/null \
    || $SUDO chsh -s "$zsh_path" "$USER_NAME" 2>/dev/null \
    || warn "couldn't change shell automatically — run: chsh -s $zsh_path"
}

# --- nvim plugin sync ------------------------------------------------------
sync_nvim() {
  [ -n "${NO_NVIM_SYNC:-}" ] && return 0
  have nvim || return 0
  say "Installing Neovim plugins (headless, minimal profile)"
  # NVIM_MINIMAL here too: this bash process doesn't source the linked zsh
  # fragment, and without it the sync would pull the full mason/LSP stack.
  NVIM_MINIMAL=1 nvim --headless "+Lazy! sync" +qa >/dev/null 2>&1 \
    || warn "plugin sync incomplete — just open nvim to finish."
}

# ---------------------------------------------------------------------------
SETUP_RC=0
install_packages
nvim_recent || install_nvim_tarball
install_ohmyzsh
install_starship
install_uv
ensure_repo
install_dotfile_commands
link_dotfiles
set_login_shell
sync_nvim

echo
if [ "${SETUP_RC:-0}" -ne 0 ]; then
  warn "dotfile linking reported conflicts (see above) — resolve and re-run: $DOTFILES_DIR/bootstrap-vps.sh"
fi
say "Done. Start a new session (or run: exec zsh) to pick up the new shell."
