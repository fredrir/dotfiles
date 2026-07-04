# dotfiles

<!-- fastfetch:start -->

```
                                           fredrir @ archpc
                    -`                     ───────────────────────────────
                   .o+`
                  `ooo/                      SYSTEM
                 `+oooo:                   󰣇  OS        Arch Linux x86_64
                `+oooooo:                  󰌽  Kernel    Linux 7.0.14-arch1-1
                -+oooooo+:                 󰅐  Uptime    2 hours, 9 mins
              `/:-:++oooo+:                󰏗  Packages  1031 (pacman)
             `/++++/+++++++:               󰆍  Shell     zsh 5.9.1
            `/++++++++++++++:
           `/+++ooooooooooooo/`              HARDWARE
          ./ooosssso++osssssso+`           󰻠  CPU       AMD Ryzen 7 9800X3D (16) @ 5.27 GHz
         .oossssso-````/ossssss+`          󰢮  GPU       NVIDIA GeForce RTX 5070 Ti [Discrete]
        -osssssso.      :ssssssso.         󰍛  Memory    14 GB / 31 GB [46%]
       :osssssss/        osssso+++.        󰋊  Disk      /  57 GB / 78 GB [73%]
      /ossssssss/        +ssssooo/-        󰋊  Disk      /home  75 GB / 118 GB [63%]
    `/ossssso+/:-        -:/+osssso+-
   `+sso+:-`                 `.-/+oso:       DESKTOP
  `++:.                           `-/+/    󰧨  DE        KDE Plasma 6.7.2
  .`                                 `/    󰖯  WM        KWin (Wayland)
                                           󰆌  Terminal  terminal
                                           󰏘  Theme     Breeze (Dark) [Qt]
                                           󰍹  Display   2560x1440 in 27", 144 Hz [External] *
                                           󰍹  Display   2560x1440 in 27", 144 Hz [External]

                                             NETWORK
                                           󰗊  Locale    C

                                             ● ● ● ● ● ● ● ●
```

<!-- fastfetch:end -->

## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh desktop/arch-linux/kde            # archpc
./setup.sh laptop/arch-linux/hyprland        # laptop
./setup.sh macbook/macos                     # mac
```

## VPS / headless server

```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/fredrir/dotfiles/main/bootstrap-vps.sh)"
```

Already cloned, or re-running

```bash
~/dotfiles/bootstrap-vps.sh            # full bootstrap
./setup.sh vps/linux                   # just re-stow the configs
```

Stows `shared` + `linux/server` only. The profile exports `NVIM_MINIMAL=1`, so
nvim runs in minimal mode — same editor, treesitter, telescope, git and theme,
but no LSP/mason/formatter/AI/DB/debug plugins, so nothing pulls node, Go or a
language-server toolchain (~400 MB instead of >1 GB). Everything is env-gated in
`shared/nvim`; unset `NVIM_MINIMAL` for the full IDE. Knobs: `NO_CHSH=1`,
`NO_NVIM_SYNC=1`, `DOTFILES_DIR` / `DOTFILES_REPO`.

## Adding a config

Placeholders:
- `<group>` → `linux/common`
- `<app>` → `nvim`
- `<profile>` → `desktop/arch-linux/kde`
- `<machine>` → `desktop`

```bash
cd ~/dotfiles
mkdir -p <group>/<app>/.config/<app>
mv ~/.config/<app> <group>/<app>/.config/
grep -q '<group>' hosts/<profile>/manifest || echo '<group>' >> hosts/<profile>/manifest
./setup.sh <profile>
```

## Adding a machine

1. Pick the matching profile under `hosts/<machine>/<distro>/<desktop>`

2. (Hyprland) put its monitors / GPU env / scale in
   `hosts/<profile>/hypr-local.conf`. See
   `hosts/desktop/arch-linux/kde-hyprland` (archpc) and
   `hosts/laptop/arch-linux/hyprland` (laptop).
3. `./setup.sh <profile>`

## Per-machine settings

- **Monitors / GPU / scale (Hyprland):** `hosts/<profile>/hypr-local.conf`
  (sourced last, overrides shared `conf.d/env.conf`).
- **Shell:** shared zsh fragments in `shared/zsh`; Linux-only in
  `linux/common/zsh`; Hyprland-only in `linux/hyprland/zsh`; macOS-only in
  `macos/zsh`. All co-stow into `~/.config/zsh/conf.d/` and load by numeric order.
- **Wallpaper:** place at `~/.config/hypr/wallpaper.png`

## Jetbrains

```bash
cd ~/dotfiles/linux/common
stow --adopt --no-folding --target="$HOME" --restow jetbrains
git -C ~/dotfiles checkout -- linux/common/jetbrains
/opt/WebStorm/bin/webstorm installPlugins \
    com.nasller.CodeGlancePro ru.adelf.idea.dotenv com.github.copilot \
    org.intellij.plugins.hcl "Key Promoter X"
```
