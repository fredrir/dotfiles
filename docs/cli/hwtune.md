# hwtune

## Commands

<!-- cli:commands:start -->
| Command                 | Description                                                                                  |
| ----------------------- | -------------------------------------------------------------------------------------------- |
| `hwtune`                | Benchmarks hardware, applies tuning, and checks stability.                                   |
| `hwtune bios`           | Imports, compares, and verifies BIOS setting exports.                                        |
| `hwtune bios import`    | Normalizes an ASUS export into config/bios/exports and diffs it against the previous one.    |
| `hwtune bios diff`      | Diffs two exports, by default the latest two.                                                |
| `hwtune bios check`     | Checks the spec against the latest export and the running system.                            |
| `hwtune bios list`      | Lists imported exports.                                                                      |
| `hwtune status`         | Shows daemons, CPU policy, fans, GPU, and kernel error counts.                               |
| `hwtune stress`         | Runs a stability test while sampling temperatures and fans.                                  |
| `hwtune stress cpu`     | Runs stress-ng on all cores, at light load, or one core at a time.                           |
| `hwtune stress mem`     | Runs a memory verification with stress-ng or memtester.                                      |
| `hwtune stress gpu`     | Loops a GPU benchmark and watches for Xid errors.                                            |
| `hwtune sample`         | Samples temperatures and fans for a while and prints the peaks.                              |
| `hwtune tune`           | Plans, validates, and applies operating-system tuning profiles.                              |
| `hwtune tune plan`      | Shows supported controls and proposed trials without applying settings.                      |
| `hwtune tune auto`      | Benchmarks candidate settings and restores the original settings unless --apply is selected. |
| `hwtune tune apply`     | Validates and applies the checked-out host profile.                                          |
| `hwtune bench`          | Opens the benchmark command menu or prints its help.                                         |
| `hwtune bench run`      | Measures the current machine and optionally stores the result.                               |
| `hwtune bench plan`     | Shows available suites and expected writes without measuring.                                |
| `hwtune bench report`   | Compares compatible runs grouped by BIOS and LACT configuration.                             |
| `hwtune bench show`     | Displays a stored benchmark run.                                                             |
| `hwtune bench list`     | Lists stored benchmark runs.                                                                 |
| `hwtune bench health`   | Reports warnings derived from benchmark history.                                             |
| `hwtune bench compare`  | Compares two benchmark runs.                                                                 |
| `hwtune bench trend`    | Shows one benchmark metric over time.                                                        |
| `hwtune bench baseline` | Sets, clears, or shows the baseline run for a machine and hardware configuration.            |
| `hwtune bench prune`    | Removes superseded runs while preserving baselines and configuration history.                |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                                  | Description                                                                                |
| ------------------------------------- | ------------------------------------------------------------------------------------------ |
| `--host <NAME>`                       | Selects the host for tuning context and benchmark storage.                                 |
| `--bios-version <VERSION>`            | Overrides the BIOS version read from DMI.                                                  |
| `-n`, `--dry-run`                     | Shows planned changes without writing.                                                     |
| `--no-live`                           | Skips checks that read the running system.                                                 |
| `--profile <PROFILE>`                 | Selects all-core, light, or per-core.                                                      |
| `--minutes <MINUTES>`                 | Sets the duration in minutes, per core for per-core.                                       |
| `--cores <LIST>`                      | Limits per-core to a core list such as 0-7 or 0,2,4.                                       |
| `--offset <OFFSET>`                   | Records the Curve Optimizer offset under test in the log.                                  |
| `--no-log`                            | Skips appending to the stability log.                                                      |
| `--percent <PERCENT>`                 | Sets the share of available memory to test.                                                |
| `--tool <TOOL>`                       | Selects the stress or benchmark program.                                                   |
| `--json`                              | Prints structured results as JSON.                                                         |
| `--apply`                             | Saves the validated winner as the host profile and retains its settings.                   |
| `--min-improvement <MIN_IMPROVEMENT>` | Sets the required improvement percentage in addition to the measured noise band.           |
| `--metric <METRIC>`                   | Selects the CPU validation metric or filters benchmark report metric keys.                 |
| `--max-temp <MAX_TEMP>`               | Sets the tuning temperature ceiling in Celsius; the hardware safety ceiling still applies. |
| `--stress-seconds <STRESS_SECONDS>`   | Sets the duration of each monitored CPU stability test.                                    |
| `--tier <TIER>`                       | Selects the quick, standard, or heavy benchmark tier.                                      |
| `--only <ONLY>`                       | Limits a run to a comma-separated list of measurement families.                            |
| `--workdir <WORKDIR>`                 | Selects the directory used by the disk benchmark tier.                                     |
| `--note <NOTE>`                       | Records why a benchmark run was taken.                                                     |
| `--stability <STABILITY>`             | Links a stored stability session to the benchmark run.                                     |
| `--tag <TAG>`                         | Adds a label to a benchmark run.                                                           |
| `--force`                             | Runs benchmarks despite unsuitable measurement conditions.                                 |
| `--no-save`                           | Prints a benchmark result without storing it.                                              |
| `--baseline`                          | Pins the new benchmark run as its machine's baseline.                                      |
| `--group-by <GROUP-BY>`               | Groups reports by BIOS or BIOS and LACT settings.                                          |
| `--before <BEFORE>`                   | Selects the run before a tuning change; requires --after.                                  |
| `--after <AFTER>`                     | Selects the run after a tuning change; requires --before.                                  |
| `--limit <LIMIT>`                     | Limits the number of stored runs listed.                                                   |
| `--all`                               | Includes noisy and aborted runs in a listing.                                              |
| `--keep <KEEP>`                       | Sets the number of runs retained per machine configuration.                                |
| `--yes`                               | Prunes stored runs without asking for confirmation.                                        |
| `-h`, `--help`                        | Shows help for the selected command and exits.                                             |
| `--completions <SHELL>`               | Prints a shell completion script for the named shell and exits.                            |
| `-V`, `--version`                     | Prints the version and exits.                                                              |
<!-- cli:flags:end -->

## Benchmarks

| Task | Command |
| --- | --- |
| inspect workloads | `hwtune bench plan --tier quick` |
| record a baseline | `hwtune bench run --baseline` |
| compare before and after | `hwtune bench compare <before> <after>` |
| inspect tuning context changes | `hwtune bench report --before <before> --after <after>` |
| compare BIOS and LACT configurations | `hwtune bench report` |
| inspect history health | `hwtune bench health` |
| measure without storing | `hwtune bench run --no-save --json` |

| Data | Meaning |
| --- | --- |
| BIOS and LACT settings hashes | Configuration identity; export dates do not create another group |
| BIOS and LACT source hashes | Exact imported or configured file |
| Live controls | Observed CPU policy and GPU power settings |
| Source kind | Imported BIOS export or LACT configuration; activation is unverified |

## Tuning

| Task | Command |
| --- | --- |
| inspect supported OS controls and trials | `hwtune tune plan` |
| compare candidates, then restore the original settings | `hwtune tune auto` |
| save and retain a validated winner | `hwtune tune auto --apply` |
| validate and apply the checked-out host profile | `hwtune tune apply` |
| inspect profile changes | `git diff -- config/hwtune/<host>.json` |
| restore a profile revision | `git restore --source=<commit> -- config/hwtune/<host>.json` then `hwtune tune apply` |

| Data | Path |
| --- | --- |
| Desired OS settings | `config/hwtune/<host>.json` |
| Store schema | `benchmarks/store.json` |
| Benchmark runs | `benchmarks/hosts/<host>/runs/<id>.json` |
| Baselines | `benchmarks/hosts/<host>/baselines.json` |
| Stability sessions | `benchmarks/hosts/<host>/stability/<id>.json` |
| Tuning sessions | `benchmarks/hosts/<host>/tuning/<id>.json` |

| Env | Default |
| --- | --- |
| `HWTUNE_BENCHMARKS` | `<repository>/benchmarks` |
| `HWTUNE_HOST` | Saved host, inventory alias, or local hostname |
| `HWTUNE_LACT_CONFIG` | `/etc/lact/config.yaml` |
| `HWTUNE_MEASUREMENT_LOCK` | `/tmp/hwtune-measurement.lock` |

Git records desired settings. Applying a reverted profile requires `hwtune tune apply` to change the live OS controls. Automatic tuning changes OS controls; firmware settings remain managed through BIOS exports and checks.
