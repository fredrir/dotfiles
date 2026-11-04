# doc-purge

## Commands

<!-- cli:commands:start -->
| Command     | Description                                                            |
| ----------- | ---------------------------------------------------------------------- |
| `doc-purge` | Purges comments, doc strings and typographic glyphs from source files. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                                                            |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `-t`, `--type <TYPE>`   | Limits the walk to one file type, named by extension or language; repeats or takes a comma-separated list for several. |
| `--check`               | Checks whether anything would change without writing, and exits 1 when it would.                                       |
| `--diff`                | Prints a unified diff with three lines of context and writes nothing.                                                  |
| `--stdin <FILENAME>`    | Reads standard input as the named file and writes the purged result to standard output.                                |
| `-q`, `--quiet`         | Reports only failures.                                                                                                 |
| `--json`                | Emits one JSON object per file and a summary object, one per line.                                                     |
| `--stats`               | Prints a per-extension breakdown and writes nothing.                                                                   |
| `--only <KINDS>`        | Limits purging to the named, comma-separated kinds: comments, docs or glyphs.                                          |
| `--no-glyphs`           | Leaves typographic glyphs alone.                                                                                       |
| `--explain`             | Reports every kept comment and the marker that kept it.                                                                |
| `--list-types`          | Lists the file types doc-purge reads and exits.                                                                        |
| `--aggressive`          | Removes doc strings entirely instead of keeping their first sentence.                                                  |
| `--keep-docstrings`     | Leaves doc strings alone.                                                                                              |
| `--editor`              | Filters standard input as one file; on any error passes it through unchanged and exits 0.                              |
| `--owns`                | Reads NUL-separated filenames from standard input and prints the ones doc-purge claims.                                |
| `--watch`               | Re-runs whenever a file below the targets changes.                                                                     |
| `--uncovered`           | Lists extensions and extensionless filenames below the targets that no descriptor claims.                              |
| `--dry`                 | Shows what would go and changes nothing.                                                                               |
| `-y`, `--yes`           | Runs without asking for confirmation.                                                                                  |
| `-v`, `--verbose`       | Lists every row instead of truncating sections after 12 rows.                                                          |
| `-h`, `--help`          | Shows help for the selected command and exits.                                                                         |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                                                        |
| `-V`, `--version`       | Prints the version and exits.                                                                                          |
<!-- cli:flags:end -->