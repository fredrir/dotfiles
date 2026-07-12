#!/usr/bin/env bash
set -euo pipefail

DOTFILES_REPO="${DOTFILES_REPO:-https://github.com/fredrir/dotfiles}"
DOTFILES_DIR="${DOTFILES_DIR:-$HOME/dotfiles}"
PROFILE="vps/linux"
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
  local extras=(neovim fzf ripgrep "$fd" bat eza zoxide fastfetch \
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

# --- link dotfiles -----------------------------------------------------------
run_setup() {
  # A distro-provided ~/.zshrc would conflict with the linker; move it aside once.
  if [ -e "$HOME/.zshrc" ] && [ ! -L "$HOME/.zshrc" ]; then
    warn "backing up existing ~/.zshrc -> ~/.zshrc.pre-dotfiles"
    mv "$HOME/.zshrc" "$HOME/.zshrc.pre-dotfiles"
  fi
  say "Linking profile '$PROFILE'"
  set +e; "$DOTFILES_DIR/setup.sh" "$PROFILE"; SETUP_RC=$?; set -e
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
ensure_repo
run_setup
set_login_shell
sync_nvim

echo
if [ "${SETUP_RC:-0}" -ne 0 ]; then
  warn "dotfile link reported conflicts (see above) — resolve and re-run: $DOTFILES_DIR/setup.sh $PROFILE"
fi
say "Done. Start a new session (or run: exec zsh) to pick up the new shell."
