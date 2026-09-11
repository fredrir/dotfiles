# hwtune

## Commands

<!-- cli:commands:start -->
| Command              | Description                                                                               |
| -------------------- | ----------------------------------------------------------------------------------------- |
| `hwtune`             | Checks BIOS settings declaratively and runs stability tests on the desktop host.          |
| `hwtune bios`        | Imports, compares, and verifies BIOS setting exports.                                     |
| `hwtune bios import` | Normalizes an ASUS export into config/bios/exports and diffs it against the previous one. |
| `hwtune bios diff`   | Diffs two exports, by default the latest two.                                             |
| `hwtune bios check`  | Checks the spec against the latest export and the running system.                         |
| `hwtune bios list`   | Lists imported exports.                                                                   |
| `hwtune status`      | Shows daemons, CPU policy, fans, GPU, and kernel error counts.                            |
| `hwtune stress`      | Runs a stability test while sampling temperatures and fans.                               |
| `hwtune stress cpu`  | Runs stress-ng on all cores, at light load, or one core at a time.                        |
| `hwtune stress mem`  | Runs a memory verification with stress-ng or memtester.                                   |
| `hwtune stress gpu`  | Loops a GPU benchmark and watches for Xid errors.                                         |
| `hwtune sample`      | Samples temperatures and fans for a while and prints the peaks.                           |
| `hwtune bench`       | Runs sysinfo bench tagged with the BIOS export and LACT config hashes.                    |
| `hwtune report`      | Compares the latest benchmark run per BIOS export.                                        |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                       | Description                                                     |
| -------------------------- | --------------------------------------------------------------- |
| `--host <NAME>`            | Selects the host name; the default is the short hostname.       |
| `--bios-version <VERSION>` | Overrides the BIOS version read from DMI.                       |
| `-n`, `--dry-run`          | Shows the result without writing.                               |
| `--no-live`                | Skips checks that read the running system.                      |
| `--profile <PROFILE>`      | Selects all-core, light, or per-core.                           |
| `--minutes <MINUTES>`      | Sets the duration in minutes, per core for per-core.            |
| `--cores <LIST>`           | Limits per-core to a core list such as 0-7 or 0,2,4.            |
| `--offset <OFFSET>`        | Records the Curve Optimizer offset under test in the log.       |
| `--no-log`                 | Skips appending to the stability log.                           |
| `--percent <PERCENT>`      | Sets the share of available memory to test.                     |
| `--tool <TOOL>`            | Selects the stress or benchmark program.                        |
| `--note <NOTE>`            | Records why the benchmark run was taken.                        |
| `--metric <KEY>`           | Limits the report to metrics whose key contains the text.       |
| `-h`, `--help`             | Shows help for the selected command and exits.                  |
| `--completions <SHELL>`    | Prints a shell completion script for the named shell and exits. |
| `-V`, `--version`          | Prints the version and exits.                                   |
<!-- cli:flags:end -->
