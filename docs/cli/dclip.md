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

| Name | Copy | Paste |
| --- | --- | --- |
| macie | pasteboard | pasteboard |
| archie, `WAYLAND_DISPLAY` or `DISPLAY` | Wayland or `xclip` | Wayland or `xclip` |
| mux pane without a display | OSC 52 to `/dev/tty` | origin's `dclip serve` over mutual TLS |

| Name | Value |
| --- | --- |
| Routes | `HWIRE_SESSION` route first, then the last that answered, then the rest |
| Budget | 300 ms to connect |
| Ports, relays, units | [wezterm-mux.md](../wezterm-mux.md#dclip) |