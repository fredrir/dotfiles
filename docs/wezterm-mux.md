## wezterm mux: macie ⇄ archie

Both machines run a `wezterm-mux-server`. Each dials the other over mutual TLS
on the cable, direct Wi-Fi, regular LAN, or Tailscale route, in that order. No
SSH is involved in the mux path; the LAN route borrows only the
`home-lan-connect` resolver.

Peer-facing port is 8443 on both hosts. A column that differs from its bind is
reached through `socat`.

## Host switching

| Name                                    | Value                                                                                |
| --------------------------------------- | ------------------------------------------------------------------------------------ |
| `attach_mux archie`, `attach_mux macie` | Fresh shell replaces the invoking split; sibling panes and existing sessions remain  |
| `attach_mux`                            | Fresh shell on the GUI computer's peer                                               |
| `attach_mux <ssh-host>`                 | SSH session replaces the invoking split; see [SSH domains](#ssh-domains)             |
| Request                                 | Shell emits `ATTACH_MUX`; GUI resolves its localmux pane ID                          |
| TLS layouts                             | `local_pane_layout=true`; localmux owns tabs/splits, remote tabs are not imported    |
| Return to GUI computer                  | Fresh shell in localmux's `local` domain                                             |
| Remote shell exits                      | Pane swaps back to a fresh local shell; closing the pane or tab still closes it      |
| Failure                                 | Source pane stays open; GUI reports the error                                        |
| Prerequisite                            | Updated vertical-tabs WezTerm GUI, CLI and localmux server; reload shell definitions |
| Restart                                 | Restarting localmux terminates its active sessions; save work first                  |


| Route     | macie `tls_servers` | macie peer-facing  | archie `tls_servers` | archie peer-facing   |
| --------- | ------------------- | ------------------ | -------------------- | -------------------- |
| cable     | 127.0.0.1:8443      | 10.77.77.1:8443    | 10.77.77.2:8443      | 10.77.77.2:8443      |
| wifi      | 127.0.0.1:8444      | 10.77.78.1:8443    | 10.77.78.2:8443      | 10.77.78.2:8443      |
| lan       | 127.0.0.1:8446      | `<macie-lan>`:8443 | 127.0.0.1:8446       | `<archie-lan>`:8443  |
| tailscale | 127.0.0.1:8445      | 100.75.71.79:8443  | 100.124.205.100:8443 | 100.124.205.100:8443 |

| Route     | `tls_clients` name | macie `remote_address` | archie `remote_address` |
| --------- | ------------------ | ---------------------- | ----------------------- |
| cable     | `<peer>-cable`     | 10.77.77.2:8443        | 10.77.77.1:8443         |
| wifi      | `<peer>-wifi`      | 10.77.78.2:8443        | 10.77.78.1:8443         |
| lan       | `<peer>-lan`       | 127.0.0.1:8447         | 127.0.0.1:8447          |
| tailscale | `<peer>-tailscale` | 100.124.205.100:8443   | 100.75.71.79:8443       |

## SSH domains

| Name          | Value                                                                        |
| ------------- | ---------------------------------------------------------------------------- |
| WezTerm mux   | `ssh-mux.lua` hosts; unix domain via `ssh -T <host> wezterm cli proxy`       |
| Transport     | OpenSSH: `~/.ssh/config`, ControlMaster, ProxyCommand                        |
| Remote        | WezTerm at the host's `wezterm` path; its mux server starts on demand        |
| Requires      | `wezterm.mux.local_pane_layout_domains` containing `unix` (vertical-tabs)    |
| Disconnect    | ssh drop closes the local panes; shells keep running in the remote mux       |
| Plain SSH     | every other literal `Host` in `~/.ssh/config`; `multiplexing = "None"`       |
| Excluded      | `macie`, `archie`; they attach over TLS                                      |
| Resolution    | WezTerm's SSH client at connect time; `Match exec` is unsupported there      |
| `Include`     | relative to `~/.ssh`; WezTerm does not expand `~`                            |
| ProxyCommand  | runs with localmux's `PATH`; the launchd job prepends `/opt/homebrew/bin`    |
| New host      | restart localmux; reloading the config does not register new domains         |

```console
$ attach_mux ntnu
$ attach_mux fredrir-04
```

## The LAN route

Both LAN addresses are DHCP, so neither is a literal in `hosts.lua`, and
`tls_clients` accepts no proxy command. Both ends are relayed instead.

| Name         | Value                                                                 |
| ------------ | --------------------------------------------------------------------- |
| Resolver     | `~/dotfiles/scripts/shell/home-lan-connect --resolve <peer>.local`    |
| Cache        | `~/.local/state/home-lan-connect/<peer>.local`; fresh for 1 minute    |
| Refresh      | both relays run `--refresh` every poll; stale or missing → mDNS       |
| Accepted     | both ends inside 192.168.1.0/24                                       |
| mDNS scope   | avahi denies `macie0`, `macie1` and `archie0`; see [ssh.md](ssh.md)   |
| Server relay | `<own-lan>:8443` → `127.0.0.1:8446`, `range=<peer-lan>/32`            |
| Client relay | `127.0.0.1:8447` → `<peer-lan>:8443`, sourced from `<own-lan>`        |
| Restart      | relay exits when the resolved pair moves; launchd or systemd restarts |

The subnet filter is what stops `archie.local` advertised on `archie-direct`
from masquerading as the regular LAN. `range=` is defence in depth. Mutual TLS
is the control that rejects a stranger: a client with no `CN=fredrir`
certificate is dropped with TLS alert 40 before any mux traffic.

## Connection information

Remote mux panes carry a validated `HWIRE_SESSION` environment stamp with the
origin host, destination host, selected route, and TLS marker. That lets
`hwire -i` describe the actual pane rather than whichever route is preferred
now:

```console
$ hwire -i
CABLE - TLS                                                      macie --> archie
```

`WEZTERM_HOSTNAME` is only the hostname of the process environment; whether it
is empty or set cannot identify the mux domain or prove which route carried the
connection. `hwire` therefore accepts only the validated session stamp as TLS
evidence. `hwire -iv` shows that evidence and the selected domain.

Existing remote panes predate the stamp and must be reopened once after this
change. New tabs and splits opened from a stamped TLS pane propagate the stamp
automatically. Because an unstamped legacy pane is indistinguishable from a
local pane, it is shown as local route availability instead of a guessed TLS
route.

## Why the two halves differ

`wezterm-mux-server` binds every `tls_servers` entry at startup and exits if any
one of them fails, so fixed entries would mean the server refuses to start
whenever an interface is down — which is most of the time, since the cable comes
and goes and `archie-direct` is only up on demand.

| Host   | How the binds survive an absent address                                    |
| ------ | -------------------------------------------------------------------------- |
| archie | `net.ipv4.ip_nonlocal_bind=1` — binds the real cable/wifi/tailscale anyway |
| macie  | binds loopback **ports**; one `socat` per route exposes the address        |

The LAN route is loopback on both halves: its address is DHCP, so there is no
literal to bind even with `ip_nonlocal_bind`.


## Certificates

| Check                       | Reads                  | Needs                  |
| --------------------------- | ---------------------- | ---------------------- |
| server verifying its peer   | client cert Subject CN | `CN=fredrir` (`$USER`) |
| client verifying the server | server cert SAN        | `DNS:<hostname>`       |


```
mtls ca                  # macie only, once -- the key was destroyed after signing,
                         # so re-issuing anything means a new CA on both hosts (~10 min)
mtls csr                 # on each host; its key leaves it only sops-encrypted
mtls issue <host> <csr>  # on macie, against the CSR it sent
mtls install             # on each host
mtls doctor              # both hosts, any time
mtls doctor --probe 10.77.77.2:8443 --peer-name archie
lsof -nP -iTCP -sTCP:LISTEN | grep 844          # exactly the intended addresses
```

| Name             | Value                                                          |
| ---------------- | -------------------------------------------------------------- |
| Live             | `~/.local/share/wezterm/mtls/{ca,cert,private_key}.pem`        |
| Encrypted        | `linux/arch/wezterm-mtls/`, `macos/wezterm-mtls/`              |
| Restored by      | `./setup.sh` / `dotfile sync`, once the age identity is back   |
| Age identity     | `config/age/<host>.age`; see [Reinstall](#reinstall)           |
| CA key           | offline; needed only to re-issue                               |
| Leaves expire    | 2036-09-19                                                     |

After a re-issue, replace the encrypted copy on that host:

```
rm macos/wezterm-mtls/*.enc      # linux/arch/wezterm-mtls on archie, with --arch below
for f in ca cert private_key; do
  dotfile secret add ~/.local/share/wezterm/mtls/$f.pem --macos --pkg wezterm-mtls
done
```

## Reinstall

| Step                           | Command                                        |
| ------------------------------ | ---------------------------------------------- |
| once per host, after enrolling | `dotfile secret wrap`                          |
| fresh install                  | `./setup.sh`; asks for the wrap passphrase     |
| mux certificates               | restored by the same sync                      |
| restart the mux on archie      | `systemctl --user restart wezterm-mux`         |
| check                          | `dotfile secret doctor`, `wezterm-mtls doctor` |


## Relevant files

```
# Shared
shared/wezterm/domain/hosts.lua        addresses, binds, dials, PEM paths
shared/wezterm/domain/tls.lua          tls_servers and tls_clients
shared/wezterm/bin/wezterm-mux-route   static and LAN socat relays
scripts/shell/home-lan-connect         the filtered, cached LAN pair both relays refresh
shared/wezterm/domain/unix.lua         localmux, default_domain, no_serve_automatically
scripts/shell/wezterm-mtls             CA, CSR, issue, install, doctor
shared/wezterm/domain/ssh.lua          ssh_domains
shared/wezterm/domain/ssh-hosts.lua    SSH hosts read from ~/.ssh/config
shared/wezterm/domain/ssh-mux.lua      SSH hosts with a remote WezTerm mux
shared/wezterm/utils/attach-mux.lua    the `ATTACH_MUX` handler
shared/ssh/config                      the `Include` both OpenSSH and WezTerm read
shared/wezterm/utils/hwire-session.lua propagates TLS metadata to tabs and splits
shared/zsh/49-wezterm.zsh              `mux`, the `archie`/`macie` aliases, and TLS metadata
scripts/rust/crates/mux-route/         which route answers, and the domain to attach over it
scripts/rust/crates/hostkit/           the addresses those two read, and the guard on hosts.lua

# Macie
macos/wezterm-mtls/                    encrypted key, leaf, CA
macos/launchd/com.fredrir.wezterm-mux.plist
macos/launchd/com.fredrir.wezterm-mux-route.{cable,wifi,lan,tailscale}.plist
macos/launchd/com.fredrir.wezterm-mux-dial.lan.plist

# Archie
linux/arch/wezterm-mtls/               encrypted key, leaf, CA
linux/arch/wezterm-mux/wezterm-mux.service
linux/arch/wezterm-mux/wezterm-mux-route-lan.service
linux/arch/wezterm-mux/wezterm-mux-dial-lan.service
linux/arch/wezterm-mux-sysctl/etc/sysctl.d/30-wezterm-mux.conf
```
