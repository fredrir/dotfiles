"""Coverage for defects found in review that the suite did not previously catch."""

import json
import subprocess
import sys
from unittest import mock

import pytest
from builders import SNAPSHOT, build_run, metric
from typer.testing import CliRunner

from tools.core import blocks
from tools.utils.sysinfo.bench import store
from tools.utils.sysinfo.bench.cli import app
from tools.utils.sysinfo.bench.compare import compare_metric, noise_band
from tools.utils.sysinfo.bench.conditions import gate_reasons, grade_for
from tools.utils.sysinfo.bench.health import regression_issue
from tools.utils.sysinfo.bench.record import LIB, epoch_of
from tools.utils.sysinfo.bench.suites import disk, memory


class TestNoiseBand:
    def test_a_single_sample_gets_a_wider_band_than_the_floor(self):
        # fio and glmark2 are the tools run once, and they are exactly the ones
        # that do not repeat to 2%. mad is 0 for one sample, so the band used to
        # collapse onto the floor and call a 2.1% move a regression.
        once = metric("disk.seq_read", [1000.0])

        assert once.mad == 0.0
        assert noise_band(once, once) > 2.0

    def test_a_replicated_metric_keeps_the_tight_floor(self):
        many = metric("cpu.multi", [100.0, 100.0, 100.0])

        assert noise_band(many, many) == 2.0

    def test_a_small_move_on_one_sample_is_noise_not_a_regression(self):
        left = build_run(metrics=(metric("disk.seq_read", [1000.0]),))
        right = build_run(metrics=(metric("disk.seq_read", [979.0]),))

        delta = compare_metric(left, right, left.metrics[0], right.metrics[0])

        assert delta.verdict == "noise"


class TestGrading:
    def test_job_failures_make_a_run_noisy(self):
        assert grade_for((), ["m"], ["disk: fio could not run"]) == "noisy"

    def test_a_clean_run_still_grades_clean(self):
        assert grade_for((), ["m"], []) == "clean"

    def test_no_metrics_is_still_aborted(self):
        assert grade_for((), [], []) == "aborted"


class TestWorkDirGate:
    def test_a_memory_filesystem_is_refused_when_the_tier_writes(self):
        reasons = gate_reasons({"filesystem": {"fstype": "tmpfs"}}, writes_disk=True)

        assert any("tmpfs" in reason for reason in reasons)

    def test_a_real_filesystem_passes(self):
        assert gate_reasons({"filesystem": {"fstype": "ext4"}}, writes_disk=True) == ()

    def test_no_gate_when_the_tier_writes_nothing(self):
        assert gate_reasons({"filesystem": {"fstype": "tmpfs"}}, writes_disk=False) == ()


class TestLoadRun:
    @pytest.mark.parametrize(
        ("name", "payload"),
        [
            ("list.json", b"[1, 2, 3]"),
            ("string.json", b'"hello"'),
            ("null.json", b"null"),
            ("number.json", b"12"),
            ("latin1.json", b'{"host": "archi\xe9"}'),
            ("truncated.json", b"{not json"),
        ],
    )
    def test_a_corrupt_run_is_skipped_rather_than_fatal(self, benchmarks, name, payload):
        # One bad byte in one file used to raise out of list, show, compare,
        # trend, health, prune and dotfile check alike.
        directory = store.host_dir("archie")
        directory.mkdir(parents=True, exist_ok=True)
        (directory / name).write_bytes(payload)

        assert store.load_run(directory / name) is None
        assert store.list_runs() == []


class TestEpoch:
    def drift(self, change):
        snapshot = json.loads(json.dumps(SNAPSHOT))
        change(snapshot)
        return epoch_of(snapshot)

    def test_running_as_root_does_not_change_identity(self):
        # PhysicalMemory is root-only on Linux, so modules reads 0 unprivileged
        # and 2 under sudo. One sudo run used to orphan the pinned baseline.
        assert self.drift(lambda s: s["memory"].update(modules=0)) == epoch_of(SNAPSHOT)

    def test_vram_reported_as_a_float_matches_the_same_value_as_an_int(self):
        assert self.drift(lambda s: s["gpu"][0].update(memory_total=17094934528.0)) == epoch_of(
            SNAPSHOT
        )

    def test_a_small_vram_difference_between_sources_does_not_drift(self):
        # nvidia-smi and the fastfetch fallback disagree by a few hundred MiB.
        assert self.drift(lambda s: s["gpu"][0].update(memory_total=16648896512)) == epoch_of(
            SNAPSHOT
        )

    def test_device_order_is_not_identity(self):
        two = json.loads(json.dumps(SNAPSHOT))
        two["disks"] = [
            {"name": "WDC WD20EZRZ-00Z", "size": 2000398934016},
            {"name": "KINGSTON SNVS2000G", "size": 2000398934016},
        ]
        reversed_disks = json.loads(json.dumps(two))
        reversed_disks["disks"].reverse()

        assert epoch_of(two) == epoch_of(reversed_disks)

    def test_a_real_hardware_change_still_drifts(self):
        assert self.drift(lambda s: s["gpu"][0].update(name="RTX 4090")) != epoch_of(SNAPSHOT)
        assert self.drift(lambda s: s["memory"].update(total=68719476736)) != epoch_of(SNAPSHOT)


class TestHealthWording:
    def test_a_latency_regression_reads_as_above_its_baseline(self, sample_run, slower_run):
        # 20ms -> 30ms is worse by going up. A hardcoded "below" said the
        # opposite for every lower-is-better metric.
        left = build_run(metrics=(metric("workload.nvim_startup", [20.0], proportion=LIB),))
        right = build_run(metrics=(metric("workload.nvim_startup", [30.0], proportion=LIB),))
        delta = compare_metric(left, right, left.metrics[0], right.metrics[0])

        issue = regression_issue(delta, sample_run, slower_run)

        assert delta.verdict == "worse"
        assert "50% above its baseline" in issue.title

    def test_a_throughput_regression_still_reads_as_below(self, sample_run, slower_run):
        left = build_run(metrics=(metric("cpu.multi", [100.0]),))
        right = build_run(metrics=(metric("cpu.multi", [50.0]),))
        delta = compare_metric(left, right, left.metrics[0], right.metrics[0])

        assert "50% below its baseline" in regression_issue(delta, sample_run, slower_run).title


class TestDiskJobFile:
    def test_darwin_omits_disk_util_which_fio_rejects_there(self):
        with mock.patch.object(disk.sys, "platform", "darwin"):
            options = disk.global_options("1g", "/tmp", "posixaio")

        assert not any(option.startswith("disk_util") for option in options)

    def test_linux_keeps_disk_util(self):
        with mock.patch.object(disk.sys, "platform", "linux"):
            options = disk.global_options("1g", "/tmp", "libaio")

        assert "disk_util=0" in options

    def test_write_stages_are_size_bounded_and_reads_stay_time_based(self):
        spec = disk.job_file("1g", "/tmp", "libaio", {"seq-write": "6g", "rand-write": "2g"})
        stages = {
            block.splitlines()[0]: block for block in spec.split("[")[2:] if block.strip()
        }

        assert "io_size=6g" in stages["seq-write]"]
        assert "time_based" not in stages["seq-write]"]
        assert "time_based=1" in stages["seq-read]"]

    def test_predicted_writes_covers_layout_and_every_write_stage(self):
        predicted = disk.predicted_writes("1g", {"seq-write": "6g", "rand-write": "2g"})

        assert predicted == (4 * 1024**3) + (6 * 1024**3) + (2 * 1024**3)

    def test_the_engine_that_ran_is_recorded_not_the_one_preferred(self, tmp_path):
        detail = {"engine": "libaio"}
        payload = json.dumps(
            {"jobs": [{"jobname": "seq-read", "read": {"bw_bytes": 1000000, "iops": 1.0}}]}
        )
        calls = []

        def fake_run(command, **kwargs):
            calls.append(command)
            failed = len(calls) == 1
            return subprocess.CompletedProcess(
                command, 1 if failed else 0, "" if failed else payload, "engine unavailable"
            )

        with mock.patch.object(disk, "run", fake_run):
            disk.measure("fio", "1m", str(tmp_path), {}, detail)

        assert detail["engine"] == "psync"

    def test_layout_files_are_discarded_when_parsing_fails(self, tmp_path):
        for stage, _mode, _block, _key in disk.STAGES:
            (tmp_path / f"{stage}.0.0").write_bytes(b"x" * 16)

        def fake_run(command, **kwargs):
            return subprocess.CompletedProcess(command, 0, '{"jobs": []}', "")

        with mock.patch.object(disk, "run", fake_run), pytest.raises(disk.MeasurementError):
            disk.measure("fio", "1m", str(tmp_path), {}, {})

        assert list(tmp_path.iterdir()) == []


class TestMemorySuite:
    def test_cache_and_dram_are_measured_separately(self):
        with mock.patch.object(memory, "tool_path", lambda *_n: "/usr/bin/sysbench"), \
             mock.patch.object(memory, "version_of", lambda *a, **k: "1.0.20"):
            jobs = {job.name: job for job in memory.jobs(None)}

        assert set(jobs) == {"mem.bandwidth", "mem.random", "cache.bandwidth"}
        # The DRAM job must escape cache; the cache job must sit inside it.
        assert jobs["mem.bandwidth"].detail["block"] == memory.DRAM_BLOCK
        assert jobs["cache.bandwidth"].detail["block"] == memory.CACHE_BLOCK
        assert memory.parse_size(memory.DRAM_BLOCK) > memory.parse_size(memory.CACHE_BLOCK) * 64

    def test_the_dram_job_is_world_comparable_and_the_cache_job_is_not(self):
        with mock.patch.object(memory, "tool_path", lambda *_n: "/usr/bin/sysbench"), \
             mock.patch.object(memory, "version_of", lambda *a, **k: "1.0.20"):
            jobs = {job.name: job for job in memory.jobs(None)}

        assert {out.comparable for out in jobs["mem.bandwidth"].outputs} == {"world"}
        assert {out.comparable for out in jobs["cache.bandwidth"].outputs} == {"host"}

    def test_more_than_one_thread_is_used_where_the_machine_allows(self):
        assert memory.threads_for(memory.DRAM_BLOCK) >= 1
        assert memory.threads_for("1M") == (memory.os.cpu_count() or 1)

    def test_the_method_changed_so_old_numbers_are_not_compared_to_new(self):
        old = metric("mem.read", [101315.0], method="mem.bandwidth/1.0.0")
        new = metric("mem.read", [62138.0], method="mem.bandwidth/2.0.0")
        left, right = build_run(metrics=(old,)), build_run(metrics=(new,))

        delta = compare_metric(left, right, old, new)

        assert delta.verdict == "blocked"
        assert "method changed" in delta.reason


class TestBlockComments:
    def test_a_hash_inside_a_value_is_kept(self):
        entries = blocks.scan(
            ["# a header comment", "archie {", "  CASE = Xtender #2 revision", "}"],
            comments=blocks.LINE,
        )

        assert [entry.split("=") for entry in entries if not entry.opens] == [
            ("CASE", "Xtender #2 revision")
        ]

    def test_a_whole_line_comment_is_still_stripped(self):
        entries = blocks.scan(["# header", "a {", "  k = v", "}"], comments=blocks.LINE)

        assert [entry.text for entry in entries if not entry.opens] == ["k = v"]

    def test_the_default_still_strips_trailing_comments(self):
        entries = blocks.scan(["a {", "  k = v # note", "}"])

        assert [entry.split("=") for entry in entries if not entry.opens] == [("k", "v")]

    def test_every_structural_error_renders_with_its_line_and_noun(self):
        cases = {
            blocks.UNEXPECTED_CLOSE: "hosts.dotfile:3: unexpected }",
            blocks.NESTED: "hosts.dotfile:3: nested host",
            blocks.OUTSIDE: "hosts.dotfile:3: entry outside a host",
        }
        for kind, expected in cases.items():
            assert blocks.describe(blocks.BlockError(kind, 3), "hosts.dotfile", "host") == expected

    def test_an_unterminated_block_keeps_its_line_number(self):
        # allow.py and keys.py used to drop the number entirely for this case.
        error = blocks.BlockError(blocks.UNTERMINATED, 9, "archie")

        assert blocks.describe(error, "keys.dotfile", "block") == (
            "keys.dotfile:9: missing } for archie"
        )


class TestPrune:
    def stored(self, count):
        for day in range(1, count + 1):
            store.save_run(
                build_run(
                    run_id=f"2026-08-{day:02d}T09-00-00Z-abcd1234",
                    started=f"2026-08-{day:02d}T09:00:00Z",
                    metrics=(metric("cpu.multi", [100.0]),),
                )
            )

    def test_pruning_without_confirmation_refuses_when_unattended(self, benchmarks):
        self.stored(6)
        before = len(list(store.host_dir("archie").glob("*.json")))

        result = CliRunner().invoke(app, ["prune", "--keep", "2"])

        assert result.exit_code == 1
        assert len(list(store.host_dir("archie").glob("*.json"))) == before

    def test_pruning_with_yes_removes_the_runs(self, benchmarks):
        self.stored(6)

        result = CliRunner().invoke(app, ["prune", "--keep", "2", "--yes"])

        assert result.exit_code == 0
        assert "removed" in result.stdout
        remaining = {path.stem for path in store.host_dir("archie").glob("*.json")}
        # Newest, oldest and the baseline survive; the middle is thinned.
        assert len(remaining) < 6
        assert "2026-08-06T09-00-00Z-abcd1234" in remaining
        assert "2026-08-01T09-00-00Z-abcd1234" in remaining

    def test_a_dry_run_deletes_nothing(self, benchmarks):
        self.stored(6)

        result = CliRunner().invoke(app, ["prune", "--keep", "2", "--dry-run"])

        assert result.exit_code == 0
        assert len(list(store.host_dir("archie").glob("*.json"))) == 6


class TestNonInteractive:
    @pytest.mark.parametrize(
        "arguments",
        [["compare"], ["show"], ["trend", "archie"], ["baseline", "set"]],
    )
    def test_a_menu_command_without_a_terminal_fails_loudly(self, benchmarks, arguments):
        # These printed nothing and exited 0 when piped, which reads as success.
        store.save_run(build_run(metrics=(metric("cpu.multi", [100.0]),)))

        result = CliRunner().invoke(app, arguments)

        assert result.exit_code == 1
        assert result.stdout.strip() or result.stderr.strip()


class TestPrivacy:
    def test_the_repository_root_is_not_recorded_in_a_workload_command(self):
        from tools.utils.sysinfo.bench.suites.workload import displayed

        recorded = displayed(["git", "-C", "/home/someone/dotfiles", "status"], "/home/someone/dotfiles")

        assert recorded == "git -C . status"
        assert "someone" not in recorded


class TestSevenZipVersion:
    @pytest.mark.parametrize(
        ("banner", "expected"),
        [
            ("7-Zip (z) 26.02 (arm64) : Copyright (c) 1999-2021", "26.02"),
            ("7-Zip [64] 16.02 : Copyright (c) 1999-2016 Igor Pavlov", "16.02"),
            ("7-Zip [32] 9.20  Copyright (c) 1999-2010 Igor Pavlov", "9.20"),
            ("7-Zip 21.07 : Copyright", "21.07"),
        ],
    )
    def test_the_word_size_is_not_read_as_the_version(self, banner, expected):
        import re

        from tools.utils.sysinfo.bench.suites.cpu import SEVEN_ZIP_VERSION

        assert re.search(SEVEN_ZIP_VERSION, banner).group(1) == expected


if sys.platform == "win32":  # pragma: no cover - the suite targets POSIX hosts
    pytest.skip("posix only", allow_module_level=True)
