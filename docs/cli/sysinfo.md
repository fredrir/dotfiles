# sysinfo

## Commands

| Command                                   | Description                                                                   |
| ----------------------------------------- | ----------------------------------------------------------------------------- |
| `sysinfo`                                 | Summarizes the current machine's environment and hardware.                    |
| `sysinfo bench`                           | Opens the benchmark command menu or prints its help.                          |
| `sysinfo bench run`                       | Measures the current machine and optionally stores the result.                |
| `sysinfo bench show`                      | Displays a stored benchmark run.                                              |
| `sysinfo bench list`                      | Lists stored benchmark runs.                                                  |
| `sysinfo bench health`                    | Reports warnings derived from benchmark history.                              |
| `sysinfo bench compare`                   | Compares two benchmark runs.                                                  |
| `sysinfo bench trend`                     | Shows one benchmark metric over time.                                         |
| `sysinfo bench baseline set\|clear\|show` | Sets, clears, or shows baseline runs for machine configurations.              |
| `sysinfo bench document`                  | Regenerates the benchmark documentation from stored runs.                     |
| `sysinfo bench prune`                     | Removes superseded runs while preserving baselines and configuration history. |

## Flags

| Flag              | Description                                                     |
| ----------------- | --------------------------------------------------------------- |
| `-p`, `--pretty`  | Shows the complete branded hardware presentation.               |
| `-f`, `--full`    | Includes the extended hardware inventory.                       |
| `-hh`, `--health` | Explains active errors and warnings.                            |
| `--tier`          | Selects the quick, standard, or heavy benchmark tier.           |
| `--only`          | Limits a run to a comma-separated list of measurement families. |
| `--note`          | Records why a benchmark run was taken.                          |
| `--tag`           | Adds a label to a benchmark run.                                |
| `--host`          | Selects the host to record, list, assess, or prune.             |
| `--workdir`       | Selects the directory used by the disk benchmark tier.          |
| `--force`         | Runs benchmarks despite unsuitable measurement conditions.      |
| `--no-save`       | Prints a benchmark result without storing it.                   |
| `--baseline`      | Pins the new benchmark run as its machine's baseline.           |
| `--json`          | Emits a run or comparison as JSON.                              |
| `--limit`         | Limits the number of stored runs listed.                        |
| `--all`           | Includes noisy and aborted runs in a listing.                   |
| `--keep`          | Sets the number of runs retained per machine configuration.     |
| `--dry-run`       | Reports which runs would be pruned without deleting them.       |
| `--yes`           | Prunes stored runs without asking for confirmation.             |
| `--help`          | Shows help for the selected command and exits.                  |
