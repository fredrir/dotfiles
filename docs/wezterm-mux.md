## wezterm mux: macie ⇄ archie

Both machines run a `wezterm-mux-server`. Each dials the other over mutual TLS,
on the same three routes `ssh` uses. No SSH is involved in the mux path.

```
macie                                        archie
  tls_servers  127.0.0.1:8443 ←socat← 10.77.77.1:8443  ⇄  10.77.77.2:8443
               127.0.0.1:8444 ←socat← 10.77.78.1:8443  ⇄  10.77.78.2:8443
               127.0.0.1:8445 ←socat← 100.75.71.79:8443 ⇄ 100.126.231.24:8443

  tls_clients  archie-cable · archie-wifi · archie-tailscale
               (mirrored on archie as macie-*)
```

Peer-facing port is 8443 on both hosts; a client always dials `<peer>:8443`.

## Why the two halves differ

`wezterm-mux-server` binds every `tls_servers` entry at startup and exits if any
one of them fails, so three entries would mean the server refuses to start
whenever an interface is down — which is most of the time, since the cable comes
and goes and `archie-direct` is only up on demand.

| Host   | How three binds survive an absent address                                 |
| ------ | ------------------------------------------------------------------------- |
| archie | `net.ipv4.ip_nonlocal_bind=1` — binds the real addresses regardless       |
| macie  | binds three loopback **ports**; one `socat` per route exposes the address |


## Certificates

| Check                       | Reads                  | Needs                  |
| --------------------------- | ---------------------- | ---------------------- |
| server verifying its peer   | client cert Subject CN | `CN=fredrir` (`$USER`) |
| client verifying the server | server cert SAN        | `DNS:<hostname>`       |


```
mtls ca                  # macie only, once -- the key was destroyed after signing,
                         # so re-issuing anything means a new CA on both hosts (~10 min)
mtls csr                 # on each host; its key never leaves it
mtls issue <host> <csr>  # on macie, against the CSR it sent
mtls install             # on each host
mtls doctor              # both hosts, any time
mtls doctor --probe 10.77.77.2:8443 --peer-name archie
lsof -nP -iTCP -sTCP:LISTEN | grep 844          # exactly the intended addresses
```


## Relevant files

```
# Shared
shared/wezterm/domain/hosts.lua        addresses, binds, PEM paths
shared/wezterm/domain/tls.lua          tls_servers and tls_clients
shared/wezterm/domain/unix.lua         localmux, default_domain, no_serve_automatically
shared/wezterm/bin/wezterm-mtls        CA, CSR, issue, install, doctor

# Macie
macos/launchd/com.fredrir.wezterm-mux.plist
macos/launchd/com.fredrir.wezterm-mux-route.{cable,wifi,tailscale}.plist

# Archie
linux/arch/wezterm-mux/wezterm-mux.service
linux/arch/wezterm-mux-sysctl/etc/sysctl.d/30-wezterm-mux.conf
```