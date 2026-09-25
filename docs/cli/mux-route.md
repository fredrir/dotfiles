# mux-route

## Commands

<!-- cli:commands:start -->
| Command     | Description                                                   |
| ----------- | ------------------------------------------------------------- |
| `mux-route` | Prints the WezTerm mux domain for the best route to the peer. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                        |
| ----------------------- | ---------------------------------------------------------------------------------- |
| `-l`, `--list`          | Prints every route with its state, peer address, and domain instead of one domain. |
| `-h`, `--help`          | Shows help for the selected command and exits.                                     |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                    |
| `-V`, `--version`       | Prints the version and exits.                                                      |
<!-- cli:flags:end -->

## Domain

| Name          | Value                                              |
| ------------- | -------------------------------------------------- |
| Domain        | `<peer>-<route>`, such as `archie-cable`           |
| Routes probed | `cable`, `wifi`, `lan`, `tailscale`, in that order |
| Port          | 8443                                               |
| `HOST`        | `macie` or `archie`; the peer when omitted         |

The first route that answers is the one printed. Nothing answering is a
failure, as is naming this machine: its panes are already in `localmux`.

## Listing

```console
$ mux-route --list
down  cable      10.77.77.2:8443
down  wifi       10.77.78.2:8443
up    lan        192.168.50.248:8443 archie-lan
up    tailscale  100.124.205.100:8443 archie-tailscale
```

Columns are state, route, peer socket, and the domain to attach over. A route
that is down carries no domain. The `lan` peer address is resolved per probe by
`home-lan-connect`, so it moves with the DHCP lease. `attach_mux` in
`shared/zsh/49-wezterm.zsh` is the caller: see
[wezterm-mux.md](../wezterm-mux.md).
