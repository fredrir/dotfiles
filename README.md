# dotfiles

<!-- fastfetch:start -->

```
                                      fredrir @ 173
                                      ───────────────────────────────

                                        SYSTEM
                                      󰌽  OS        macOS Tahoe 26.5.1 (25F80) arm64
                        ..'           󰒓  Kernel    Darwin 25.5.0
                    ,xNMM.            󰅐  Uptime    3 days, 1 hour, 24 mins
                  .OMMMMo             󰏗  Packages  61 (brew), 5 (brew-cask)
                  lMM"                󰆍  Shell     zsh 5.9
        .;loddo:.  .olloddol;.
      cKMMMMMMMMMMNWMMMMMMMMMM0:        HARDWARE
    .KMMMMMMMMMMMMMMMMMMMMMMMWd.      󰻠  CPU       Apple M5 Pro (5+10) @ 4.61 GHz
    XMMMMMMMMMMMMMMMMMMMMMMMX.        󰢮  GPU       Apple M5 Pro (16) @ 1.62 GHz [Integrated]
   ;MMMMMMMMMMMMMMMMMMMMMMMM:         󰍛  Memory    18 GB / 24 GB [76%]
   :MMMMMMMMMMMMMMMMMMMMMMMM:         󰋊  Disk      /  127 GB / 926 GB [14%]
   .MMMMMMMMMMMMMMMMMMMMMMMMX.
    kMMMMMMMMMMMMMMMMMMMMMMMMWd.        DESKTOP
    'XMMMMMMMMMMMMMMMMMMMMMMMMMMk     󰖯  WM        Quartz Compositor 1.600.0
     'XMMMMMMMMMMMMMMMMMMMMMMMMK.     󰆌  Terminal  kitty 0.48.2
       kMMMMMMMMMMMMMMMMMMMMMMd       󰏘  Theme     Liquid Glass
        ;KMMMMMMMWXXWMMMMMMMk.        󰍹  Display   3024x1964 @ 2x in 14", 120 Hz [Built-in]
          "cooc*"    "*coo'"
                                        NETWORK
                                      󰗊  Locale    en_US.UTF-8

                                        ● ● ● ● ● ● ● ●
```

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
dotfile check
dotfile check --all
dotfile packages
dotfile format

dotfile secret scan
dotfile secret scan --staged
dotfile secret scan --commits origin/main..HEAD

dotfile secret init
dotfile secret enroll archpc
dotfile secret keys
dotfile secret sync --rewrap
dotfile secret doctor

dotfile secret add ~/.ssh/config --pkg ssh
dotfile secret edit shared/ssh/config.enc
dotfile secret status
dotfile secret apply
dotfile secret clean

dotfile secret edit facts.enc.yaml
dotfile secret facts
dotfile secret facts --unused
```

## Command-line tools

```bash
./setup.sh --commands-only
```

The public commands are declared in `scripts/pyproject.toml` and installed as
an editable uv tool in `~/.local/bin`. The project environment remains
isolated from the shell PATH.

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
