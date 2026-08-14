#!/usr/bin/env bash

sysinfo_bench() {
  require_sandboxed_home
  OUTPUT="$(SYSINFO_BENCHMARKS="$REPO/benchmarks" SYSINFO_CONFIG="$REPO/config/hosts.dotfile" \
    "$SOURCE_ROOT/scripts/python/.venv/bin/sysinfo" bench "$@" 2>&1)"
  STATUS=$?
  return 0
}

mkhosts() {
  cat > "$REPO/config/hosts.dotfile" <<'EOF'
archie {
  hostnames = archpc, archie
  role = desktop
  CPU_COOLER = Noctua NH-D15
}

macie {
  hostnames = macie
  role = laptop
}
EOF
}

mkrun() {
  local host="$1" id="$2" started="$3" cpu="$4" gpu="${5-NVIDIA GeForce RTX 5070 Ti}"
  mkdir -p "$REPO/benchmarks/$host"
  cat > "$REPO/benchmarks/$host/$id.json" <<EOF
{
  "schema": 1,
  "run_id": "$id",
  "host": "$host",
  "started": "$started",
  "tier": "quick",
  "grade": "clean",
  "note": "",
  "tags": [],
  "dotfiles_sha": "abc1234",
  "gate_reasons": [],
  "bytes_written": 0,
  "snapshot": {
    "cpu": {"model": "AMD Ryzen 7 9800X3D", "cores_physical": 8, "cores_logical": 16},
    "gpu": [{"name": "$gpu", "memory_total": 17094934528}],
    "memory": {"total": 34359738368, "modules": 2},
    "disks": [{"name": "KINGSTON SNVS2000G", "size": 2000398934016}]
  },
  "install": {"os": "arch", "kernel": "7.1.8"},
  "conditions": {"on_battery": false},
  "metrics": [
    {
      "key": "cpu.multi",
      "method": "cpu.multi/1.0.0",
      "tool": "7z",
      "tool_version": "26.02",
      "scale": "MIPS",
      "proportion": "HIB",
      "comparable": "world",
      "times_to_run": 3,
      "samples": [$cpu, $cpu, $cpu],
      "median": $cpu,
      "mad": 0.0,
      "rsd_pct": 0.0
    }
  ]
}
EOF
}

test_lists_stored_runs() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  sysinfo_bench list
  assert_ok
  assert_output_has "archie"
  assert_output_has "2026-08-01"
}

test_reports_nothing_when_no_runs_exist() {
  mkhosts
  sysinfo_bench list
  assert_ok
  assert_output_has "no runs recorded"
}

test_compares_two_runs_and_calls_a_small_change_noise() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun archie run-b 2026-08-02T09:00:00Z 101000
  sysinfo_bench compare archie:run-a archie:run-b
  assert_ok
  assert_output_has "within noise"
}

test_reports_a_real_drop_as_worse() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun archie run-b 2026-08-02T09:00:00Z 60000
  sysinfo_bench compare archie:run-a archie:run-b
  assert_ok
  assert_output_has "worse"
}

test_annotates_a_hardware_change_between_runs() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000 "NVIDIA GeForce RTX 3080"
  mkrun archie run-b 2026-08-02T09:00:00Z 100000 "NVIDIA GeForce RTX 5070 Ti"
  sysinfo_bench compare archie:run-a archie:run-b
  assert_ok
  assert_output_has "hardware changed"
  assert_output_has "RTX 3080"
}

test_refuses_to_compare_a_metric_across_machines_when_host_scoped() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun macie run-b 2026-08-02T09:00:00Z 100000
  sysinfo_bench compare archie:run-a macie:run-b
  assert_ok
  assert_output_lacks "hardware changed"
}

test_pins_and_clears_a_baseline() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  sysinfo_bench baseline set archie:run-a
  assert_ok
  assert_output_has "baseline for archie"
  grep -qF "run-a" "$REPO/benchmarks/baselines.dotfile" || fail "baseline not written"
  sysinfo_bench baseline show
  assert_output_has "run-a"
}

test_reports_a_regression_against_the_baseline_as_a_health_finding() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun archie run-b 2026-08-02T09:00:00Z 60000
  sysinfo_bench baseline set archie:run-a
  sysinfo_bench health --host archie
  assert_ok
  assert_output_has "below its baseline"
}

test_is_silent_when_the_series_is_healthy() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun archie run-b 2026-08-02T09:00:00Z 100500
  sysinfo_bench baseline set archie:run-a
  sysinfo_bench health --host archie
  assert_ok
  assert_output_has "no benchmark findings"
}

test_shows_a_metric_over_time() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun archie run-b 2026-08-02T09:00:00Z 120000
  sysinfo_bench trend archie cpu.multi
  assert_ok
  assert_output_has "higher is better"
  assert_output_has "100 000"
}

test_generates_the_benchmark_document() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  sysinfo_bench document
  assert_ok
  grep -qF "do not edit" "$REPO/benchmarks/BENCHMARKS.md" || fail "header missing"
  grep -qF "9800X3D" "$REPO/benchmarks/BENCHMARKS.md" || fail "hardware missing"
  grep -qF "cpu.multi" "$REPO/benchmarks/BENCHMARKS.md" || fail "metric missing"
}

test_rejects_an_unknown_tier() {
  mkhosts
  sysinfo_bench run --tier enormous --host archie
  assert_fails
  assert_output_has "unknown tier"
}

test_rejects_an_unknown_family() {
  mkhosts
  sysinfo_bench run --only nonsense --host archie
  assert_fails
  assert_output_has "unknown family"
}

test_rejects_an_unknown_host() {
  mkhosts
  sysinfo_bench run --host nowhere
  assert_fails
  assert_output_has "unknown host"
}

test_reports_a_malformed_hosts_file() {
  printf 'archie {\n  role = desktop\n' > "$REPO/config/hosts.dotfile"
  sysinfo_bench list --host archie
  assert_ok
  sysinfo_bench run --host archie
  assert_fails
  assert_output_has "missing }"
}

test_prune_keeps_everything_below_the_threshold() {
  mkhosts
  mkrun archie run-a 2026-08-01T09:00:00Z 100000
  mkrun archie run-b 2026-08-02T09:00:00Z 100000
  sysinfo_bench prune --dry-run
  assert_ok
  assert_output_has "nothing to prune"
}
