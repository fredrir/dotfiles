# doc-purge

## Commands

<!-- cli:commands:start -->
| Command     | Description                                                            |
| ----------- | ---------------------------------------------------------------------- |
| `doc-purge` | Purges comments, doc strings and typographic glyphs from source files. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                                                                   |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `-t`, `--type <TYPE>`   | Limits the walk to one file type, named by extension or by language, and repeats or takes a comma-separated list for several. |
| `--check`               | Check whether anything would change; write nothing and exit 1 when it would                                                   |
| `--diff`                | Print a unified diff with three lines of context; write nothing                                                               |
| `--stdin <FILENAME>`    | Read standard input as the named file and write the purged result to standard output; never writes files                      |
| `-q`, `--quiet`         | Report only failures                                                                                                          |
| `--json`                | Emit one JSON object per file and a summary object, one per line; write nothing                                               |
| `--stats`               | Print a per-extension breakdown; write nothing                                                                                |
| `--only <KINDS>`        | Limit purging to the named, comma-separated kinds: comments, docs, glyphs. Overrides --no-glyphs and --keep-docstrings        |
| `--no-glyphs`           | Leave typographic glyphs alone. Ignored when --only is given                                                                  |
| `--explain`             | Report kept comments and the marker that kept each one; write nothing                                                         |
| `--list-types`          | List the file types doc-purge reads and exit                                                                                  |
| `--aggressive`          | Remove doc strings entirely instead of keeping their first sentence. Only applies when doc strings are being purged           |
| `--keep-docstrings`     | Leave doc strings alone. Ignored when --only is given                                                                         |
| `--editor`              | Filter standard input as one file; on any error pass it through unchanged and exit 0. Takes one TARGET or --stdin FILENAME    |
| `--owns`                | Read NUL-separated filenames from standard input and print the ones doc-purge claims, one per line                            |
| `--watch`               | Re-run whenever a file below the targets changes; polls modification times every 300ms. Stop with Ctrl-C                      |
| `--uncovered`           | List extensions and extensionless filenames below the targets that no language descriptor claims, most frequent first         |
| `--dry`                 | Shows what would go without writing anything.                                                                                 |
| `-y`, `--yes`           | Runs without asking for confirmation.                                                                                         |
| `-v`, `--verbose`       | Lists every row instead of truncating sections after 12 rows.                                                                 |
| `-h`, `--help`          | Shows help for the selected command and exits.                                                                                |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                                                               |
| `-V`, `--version`       | Prints the version and exits.                                                                                                 |
<!-- cli:flags:end -->