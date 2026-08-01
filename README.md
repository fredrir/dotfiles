# dotfiles

<!-- fastfetch:start -->

`````
                                                fredrir @ archpc
                                                ───────────────────────────────

                                                  SYSTEM
                                                󰣇  OS        Arch Linux x86_64
                        -`                      󰌽  Kernel    Linux 7.1.3-arch1-2
                       .o+`                     󰅐  Uptime    31 mins
                      `ooo/                     󰏗  Packages  1087 (pacman)
                     `+oooo:                    󰆍  Shell     zsh 5.9.1
                    `+oooooo:
                    -+oooooo+:                    HARDWARE
                  `/:-:++oooo+:                 󰻠  CPU       AMD Ryzen 7 9800X3D (16) @ 5.27 GHz
                 `/++++/+++++++:                󰢮  GPU       NVIDIA GeForce RTX 5070 Ti [Discrete]
                `/++++++++++++++:               󰍛  Memory    9 GB / 31 GB [29%]
               `/+++ooooooooooooo/`             󰋊  Disk      /  63 GB / 78 GB [81%]
              ./ooosssso++osssssso+`            󰋊  Disk      /home  108 GB / 118 GB [92%]
             .oossssso-````/ossssss+`
            -osssssso.      :ssssssso.            DESKTOP
           :osssssss/        osssso+++.         󰧨  DE        KDE Plasma 6.7.2
          /ossssssss/        +ssssooo/-         󰖯  WM        KWin (Wayland)
        `/ossssso+/:-        -:/+osssso+-       󰆌  Terminal  konsole 26.04.3
       `+sso+:-`                 `.-/+oso:      󰏘  Theme     Breeze (Dark) [Qt]
      `++:.                           `-/+/     󰍹  Display   2560x1440 in 27", 144 Hz [External] *
      .`                                 `/     󰍹  Display   2560x1440 in 27", 144 Hz [External]

                                                  NETWORK
                                                󰗊  Locale    en_US.UTF-8

                                                  ● ● ● ● ● ● ● ●
`````

<!-- fastfetch:end -->

## Install

```bash
git clone https://github.com/fredrir/dotfiles ~/dotfiles
cd ~/dotfiles
./setup.sh
```


## VPS / headless server

```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/fredrir/dotfiles/main/bootstrap-vps.sh)"
```

Already cloned, or re-running

```bash
~/dotfiles/bootstrap-vps.sh
```

## Theme

- **Palette:** `theme/palette.toml

- **To regenerate theme:**

```bash
generate-theme
```

```bash
systemctl --user restart plasma-plasmashell
```

## The dotfile command

```bash
dotfile add waybar
dotfile add --description "Status bar" waybar
dotfile add --linux zsh/conf.d/11-linux-env
dotfile add --kde konsolerc
dotfile add --pkg zsh ~/.zshrc
dotfile remove linux/common/fontconfig
dotfile remove /linux/server/zsh/conf.d/10-nvim.server.zsh

dotfile link
dotfile link arch-linux/kde
dotfile link --override linux/hyprland=laptop
dotfile link -n

dotfile status
dotfile packages
dotfile format
```

## Command-line tools

Workstation commands live in a uv-managed Python project under `scripts/` and
are installed by `./setup.sh`:

```bash
uv sync --project scripts --locked
```

`scripts/.venv/bin` is added to PATH by the workstation zsh profiles and
provides `count`, `size`, `path`, `tardirs`, `gpp`, `oc`, `sysinfo`,
`power-menu`, `confirm-exit`, `clean-paste`, `generate-theme`,
`update-readme-fastfetch`, and `dotfile`.

## Tests

```bash
tests/run.sh
tests/run.sh link
uv run --project scripts pytest
```

## Adding a machine

1. Run `./setup.sh` and pick the environment under `environment/<os>/<desktop>`
2. If the environment includes a group with an `overrides/` directory (machine-specific config such as `linux/hyprland/overrides/desktop` and `linux/hyprland/overrides/laptop`), pick the override that matches the machine, or `none`

## Jetbrains

New files WebStorm creates stay in `~/.config/JetBrains`

```bash
dotfile add --linux --pkg jetbrains JetBrains/WebStorm2026.1/options/editor.xml
/opt/WebStorm/bin/webstorm installPlugins \
    com.nasller.CodeGlancePro ru.adelf.idea.dotenv com.github.copilot \
    org.intellij.plugins.hcl "Key Promoter X"
```
