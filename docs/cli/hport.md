# hport

## Commands

<!-- cli:commands:start -->
| Command           | Description                                                                              |
| ----------------- | ---------------------------------------------------------------------------------------- |
| `hport`           | Shows the peer's ports this machine forwards, with their URLs.                           |
| `hport daemon`    | Forwards the peer's listening ports until stopped; run by launchd or systemd.            |
| `hport listeners` | Prints this machine's forwardable listeners as JSON; the peer's daemon runs it over SSH. |
| `hport setup`     | Maps the peer name to 127.0.0.2, adds the macOS loopback alias, and starts the daemon.   |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                     |
| ----------------------- | --------------------------------------------------------------- |
| `--json`                | Prints the daemon state as JSON.                                |
| `-w`, `--watch`         | Prints the listeners again on every change.                     |
| `-n`, `--dry-run`       | Shows the missing setup steps without applying them.            |
| `-h`, `--help`          | Shows help for the selected command and exits.                  |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits. |
| `-V`, `--version`       | Prints the version and exits.                                   |
<!-- cli:flags:end -->

## Addresses

```
macie                                                       archie
http://archie:5173     → 127.0.0.2:5173        ┐
http://localhost:5173  → 127.0.0.1, [::1]:5173 ┴─ ssh master ─→ localhost:5173
```

| Name | Value |
| --- | --- |
| `http://<peer>:PORT` | `127.0.0.2:PORT`, always |
| `http://localhost:PORT` | `127.0.0.1:PORT` and `[::1]:PORT`, while the port is free on this machine |
| Route | the one `ssh <peer>` picks; reconnects when a better one comes up |
| Direction | both: macie imports archie's ports, archie imports macie's |
| Browser | type the scheme the first time: `http://archie:5173` |

## Forwarded

| Name | Value |
| --- | --- |
| Listeners | TCP on loopback or a wildcard address, owned by this user or by Docker's `docker-proxy` |
| Ports | `1` to `max_port`, below the ephemeral range |
| Skipped | `ssh`-owned sockets, `ignore_ports`, `ignore_processes` |
| Detection | the peer rescans every 500 ms and streams changes over the same master |

## Config

| Name | Value |
| --- | --- |
| Installed | `~/.config/hport/config.toml` |
| Source | `shared/hport/config.toml` |
| Reload | restart the daemon |

| Key | Default |
| --- | --- |
| `max_port` | `32767` |
| `ignore_ports` | `[]` |
| `ignore_processes` | `[]` |

| Env | Default |
| --- | --- |
| `HPORT_CONFIG` | `${XDG_CONFIG_HOME:-~/.config}/hport/config.toml` |
| `XDG_STATE_HOME` | `~/.local/state`; holds `hport/state.json` and `hport/master.sock` |
| `__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS` | `$HOST`, from `shared/zsh/04-env.zsh` |

## Setup

Once per machine, after `dotfile sync`:

```console
$ hport setup --dry-run
$ hport setup
```

| Step | macie | archie |
| --- | --- | --- |
| Name | `127.0.0.2 archie` in `/etc/hosts` | `127.0.0.2 macie` in `/etc/hosts` |
| Loopback alias | `/Library/LaunchDaemons/com.fredrir.hport.alias.plist` | none; `127.0.0.0/8` is local |
| Daemon | `~/Library/LaunchAgents/com.fredrir.hport.plist` | `~/.config/systemd/user/hport.service` |

## Status

```console
$ hport
archie → macie  cable
PORT  PROCESS  ARCHIE              LOCALHOST
5173  bun      http://archie:5173  http://localhost:5173
8080  java     http://archie:8080  busy (idea)
```

| Cell | Meaning |
| --- | --- |
| URL | forwarded |
| `busy (<process>)` | port held on this machine |
| `pending` | not attempted yet |
| `failed` | refused by ssh; retried every 10 s |

## Operations

| Name | macie | archie |
| --- | --- | --- |
| Restart | `launchctl kickstart -k gui/$(id -u)/com.fredrir.hport` | `systemctl --user restart hport` |
| Log | `~/Library/Logs/hport.log` | `journalctl --user -u hport` |
