# sysinfo

## Commands

<!-- cli:commands:start -->
| Command                  | Description                                                                       |
| ------------------------ | --------------------------------------------------------------------------------- |
| `sysinfo`                | Summarizes the current machine's environment and hardware.                        |
| `sysinfo bench`          | Opens the benchmark command menu or prints its help.                              |
| `sysinfo bench run`      | Measures the current machine and optionally stores the result.                    |
| `sysinfo bench plan`     | Show available suites and expected writes without measuring                       |
| `sysinfo bench show`     | Displays a stored benchmark run.                                                  |
| `sysinfo bench list`     | Lists stored benchmark runs.                                                      |
| `sysinfo bench health`   | Reports warnings derived from benchmark history.                                  |
| `sysinfo bench compare`  | Compares two benchmark runs.                                                      |
| `sysinfo bench trend`    | Shows one benchmark metric over time.                                             |
| `sysinfo bench baseline` | Sets, clears, or shows the baseline run for a machine and hardware configuration. |
| `sysinfo bench document` | Regenerates the benchmark documentation from stored runs.                         |
| `sysinfo bench prune`    | Removes superseded runs while preserving baselines and configuration history.     |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                     |
| ----------------------- | --------------------------------------------------------------- |
| `-p`, `--pretty`        | Shows the complete branded hardware presentation.               |
| `-f`, `--full`          | Includes the extended hardware inventory.                       |
| `--health`, `-hh`       | Explains active errors and warnings.                            |
| `--json`                | Emits a run or comparison as JSON.                              |
| `--timings`             | Report probe timings to stderr                                  |
| `--tier <TIER>`         | Selects the quick, standard, or heavy benchmark tier.           |
| `--only <ONLY>`         | Limits a run to a comma-separated list of measurement families. |
| `--workdir <WORKDIR>`   | Selects the directory used by the disk benchmark tier.          |
| `--note <NOTE>`         | Records why a benchmark run was taken.                          |
| `--tag <TAG>`           | Adds a label to a benchmark run.                                |
| `--host <HOST>`         | Selects the host to record, list, assess, or prune.             |
| `--force`               | Runs benchmarks despite unsuitable measurement conditions.      |
| `--no-save`             | Prints a benchmark result without storing it.                   |
| `--baseline`            | Pins the new benchmark run as its machine's baseline.           |
| `--limit <LIMIT>`       | Limits the number of stored runs listed.                        |
| `--all`                 | Includes noisy and aborted runs in a listing.                   |
| `--keep <KEEP>`         | Sets the number of runs retained per machine configuration.     |
| `--dry-run`             | Reports which runs would be pruned without deleting them.       |
| `--yes`                 | Prunes stored runs without asking for confirmation.             |
| `-h`, `--help`          | Shows help for the selected command and exits.                  |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits. |
| `-V`, `--version`       | Prints the version and exits.                                   |
<!-- cli:flags:end -->
