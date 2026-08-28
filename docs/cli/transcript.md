# transcript

## Commands

<!-- cli:commands:start -->
| Command              | Description                                                        |
| -------------------- | ------------------------------------------------------------------ |
| `transcript`         | Archives AI agent sessions as Obsidian notes.                      |
| `transcript capture` | Wraps clipboard text as a transcript note in the vault.            |
| `transcript import`  | Imports a Claude Code or Codex session as a transcript note.       |
| `transcript list`    | Lists recent Claude Code and Codex sessions.                       |
| `transcript add`     | Tracks a project for transcript sync.                              |
| `transcript rm`      | Stops tracking a project while preserving its existing notes.      |
| `transcript migrate` | Moves existing transcript groups to their configured destinations. |
| `transcript sync`    | Syncs allowlisted Claude Code and Codex sessions into the vault.   |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                              |
| ----------------------- | ------------------------------------------------------------------------ |
| `--provider <TEXT>`     | Overrides clipboard provider detection for `capture`.                    |
| `--raw`                 | Skips secret redaction during capture, import, or sync.                  |
| `--quiet`               | Prints nothing after a successful capture or sync.                       |
| `--fallback <TEXT>`     | Selects a snapshot file when the clipboard is empty.                     |
| `--latest`              | Imports the newest available session.                                    |
| `--limit <INTEGER>`     | Sets how many sessions appear in the import picker or session list.      |
| `--tools`               | Includes tool calls in imported or synchronized notes.                   |
| `--name <TEXT>`         | Sets the tracked project name instead of deriving it from the directory. |
| `--group <TEXT>`        | Assigns a tracked project to a transcript group.                         |
| `--verbose`, `-v`       | Lists every file in the migration preview.                               |
| `--dry-run`             | Reports sync changes without writing them.                               |
| `--help`                | Shows help for the selected command and exits.                           |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.          |
<!-- cli:flags:end -->
