# dotfile


## The dotfile command

```zsh
dotfile sync
dotfile sync arch-linux/kde
dotfile sync --override linux/hyprland=laptop
dotfile sync --resolve repo
dotfile sync -n
dotfile sync -p
dotfile sync -p --to archie

dotfile add waybar
dotfile add --description "Status bar" waybar
dotfile add --linux zsh/conf.d/11-linux-env
dotfile add --kde konsolerc
dotfile add --pkg zsh ~/.zshrc
dotfile remove linux/common/fontconfig
dotfile remove /linux/server/zsh/conf.d/10-nvim.server.zsh

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

dotfile secret edit vars.enc.yaml
dotfile secret vars
dotfile secret vars --unused

dotfile system add /etc/dnsmasq-macie-usb.conf --pkg macie-usb
dotfile system status
dotfile system diff
dotfile system install
```

## dotfile sync

```
dotfile sync [PROFILE] [--override <group>=<name|none>] [-n/--dry-run]
             [--resolve skip|repo|live] [--force] [-p/--push] [--to <host>]
```


### Drift



A script, `ssh` with no tty — `--resolve` decides:

| `--resolve`      |                                                                                                                                   |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `skip` (default) | materialise every config that is not drifted, report the ones that are, exit 1                                                    |
| `repo`           | the repo wins, local changes are discarded                                                                                        |
| `live`           | the live file wins; each change is adopted back into the last layer that already defines the key, or the shared base if none does |


### Push

`-p` finishes the round trip on the other machine, so a change committed here
lands there without the manual `ssh`, `git pull`, `dotfile sync`:

```zsh
dotfile sync -p
dotfile sync -p --to archie
dotfile sync -p --force
```

| step |                                                                        |
| ---- | ---------------------------------------------------------------------- |
| 1    | sync this machine, exactly as a bare `dotfile sync` would               |
| 2    | `git push`, if the branch is ahead of its upstream                      |
| 3    | read the other machine's branch and `git status` over `ssh`             |
| 4    | `git pull` and `dotfile sync` there, on a tty, so its prompts still work |

The target is the machine in `config/hosts.dotfile` that is not this one, and
`--to` names it when the file lists more than two. The first failure stops the
run and reports it, including the far side's own exit status.

Uncommitted work on *this* machine is left alone — only commits travel. The far
side is held to a stricter standard, because a dirty tree there is what breaks
the pull: its `git status` is printed and `[Y/n]` asks whether to discard it
(`git reset --hard` and `git clean -fd`, so untracked files there go too).
`--force` answers that prompt in advance, and also passes `--resolve repo` to
both syncs. A run that is not attached to a terminal never prompts; it stops and
asks for `--force`.

A machine sitting on a different branch is an abort rather than a prompt:
pulling its own branch would report success while syncing something else.


## dotfile add

## dotfile status

```
dotfile status [PROFILE]
```

| state        |                                                                                       |
| ------------ | ------------------------------------------------------------------------------------- |
| `current`    | the live file matches                                                                 |
| `formatting` | the same document in the application's own layout. Not a failure, it counts as linked |
| `pending`    | the repo moved ahead; the next sync applies it                                        |
| `drifted`    | this machine changed keys nobody has decided                                          |
| `conflict`   | the repo and this machine changed the same key                                        |
| `missing`    | nothing at the destination                                                            |




## merge.dotfile

```
ignore  workbench.colorTheme
ignore  cSpell.*
ignore  [lua]/editor.tabSize
```

## Theme


- **To switch profile:**

```zsh
dotfile theme
```

- **To regenerate after editing a palette:**

```zsh
dotfile theme apply
```

- **To see which profile each group uses:**

```zsh
dotfile theme status
```

- **After a change that touches KDE:**

```zsh
systemctl --user restart plasma-plasmashell
```