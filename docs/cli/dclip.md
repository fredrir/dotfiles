# dclip

## Commands

<!-- cli:commands:start -->
| Command       | Description                                                                 |
| ------------- | --------------------------------------------------------------------------- |
| `dclip`       | Copies stdin to the clipboard, minus one trailing newline.                  |
| `dclip serve` | Serves this clipboard to the peer until stopped; run by launchd or systemd. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                     |
| ----------------------- | --------------------------------------------------------------- |
| `-o`, `--output`        | Prints the clipboard; without a display, the mux origin's.      |
| `-h`, `--help`          | Shows help for the selected command and exits.                  |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits. |
| `-V`, `--version`       | Prints the version and exits.                                   |
<!-- cli:flags:end -->

## Backends

First match wins:

| Name | Copy | Paste |
| --- | --- | --- |
| mux pane, `HWIRE_SESSION` from the peer | OSC 52 to `/dev/tty` | origin's `dclip serve` over mutual TLS |
| ssh, `SSH_CONNECTION` or `SSH_TTY` | OSC 52 to `/dev/tty` | `no clipboard over ssh` |
| macie, or archie with `WAYLAND_DISPLAY` or `DISPLAY` | pasteboard, Wayland or `xclip` | pasteboard, Wayland or `xclip` |
| anything else | OSC 52 to `/dev/tty` | `no clipboard` |

| Name | Value |
| --- | --- |
| Routes | `HWIRE_SESSION` route first, then the last that answered, then the rest |
| Connect | 300 ms for TCP; +1 s for TLS once the peer itself answers |
| Reply | 2.5 s after TLS |
| Ports, relays, units | [wezterm-mux.md](../wezterm-mux.md#dclip) |
