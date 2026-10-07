# dotfmt

## Commands

<!-- cli:commands:start -->
| Command  | Description                                                       |
| -------- | ----------------------------------------------------------------- |
| `dotfmt` | CLI shell; formatting and configuration actions are placeholders. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                     |
| ----------------------- | --------------------------------------------------------------- |
| `--check`               | Check formatting without writing (placeholder).                 |
| `-a`, `--add`           | Offer formatter configuration to the target (placeholder).      |
| `-s`, `--sync`          | Refresh the target's formatter configuration (placeholder).     |
| `--dialect <DIALECT>`   | Select a formatter dialect (placeholder).                       |
| `-e`, `--editor`        | Format standard input for an editor (placeholder).              |
| `--stdin <FILENAME>`    | Treat standard input as the named file (placeholder).           |
| `--owns`                | Report owned files from standard input (placeholder).           |
| `-v`, `--verbose`       | Show detailed output.                                           |
| `-q`, `--quiet`         | Report only failures.                                           |
| `-h`, `--help`          | Shows help for the selected command and exits.                  |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits. |
| `-V`, `--version`       | Prints the version and exits.                                   |
<!-- cli:flags:end -->

| Invocation | Result |
| --- | --- |
| No arguments, help, version, completions | Available |
| Formatting, check, editor, ownership, add, sync | Exit 1 with a not-implemented message; no work performed |
