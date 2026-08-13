import os
import subprocess
import sys

import pytest
from builders import build_run, metric

from tools.utils.sysinfo.bench import store

HOLD_LOCK = """
import fcntl, os, sys

handle = os.open(sys.argv[1], os.O_CREAT | os.O_RDWR, 0o644)
fcntl.flock(handle, fcntl.LOCK_EX)
print("held", flush=True)
sys.stdin.read()
"""


def test_a_saved_run_reads_back_identically(benchmarks, sample_run):
    path = store.save_run(sample_run)

    assert path.parent.name == "archie"
    assert store.load_run(path) == sample_run


def test_saving_leaves_no_partial_file_behind(benchmarks, sample_run):
    store.save_run(sample_run)

    assert list(store.host_dir("archie").glob("*.partial")) == []


def test_runs_are_listed_newest_first(benchmarks, sample_run, slower_run):
    store.save_run(sample_run)
    store.save_run(slower_run)

    assert [run.run_id for run in store.list_runs()] == [slower_run.run_id, sample_run.run_id]


def test_listing_can_filter_by_grade(benchmarks, sample_run):
    store.save_run(sample_run)
    store.save_run(build_run(run_id="noisy-run", grade="noisy"))

    assert [run.run_id for run in store.list_runs(grades=("clean",))] == [sample_run.run_id]
    assert len(store.list_runs(grades=("clean", "noisy"))) == 2


def test_an_unreadable_run_is_skipped_rather_than_fatal(benchmarks, sample_run):
    store.save_run(sample_run)
    (store.host_dir("archie") / "broken.json").write_text("{not json", encoding="utf-8")

    assert [run.run_id for run in store.list_runs()] == [sample_run.run_id]


def test_baselines_round_trip_through_their_document(benchmarks):
    store.set_baseline("archie", "abcd1234", "run-one")
    store.set_baseline("macie", "ffff0000", "run-two")

    assert store.load_baselines() == {
        "archie": {"abcd1234": "run-one"},
        "macie": {"ffff0000": "run-two"},
    }


def test_clearing_the_last_baseline_removes_the_document(benchmarks):
    store.set_baseline("archie", "abcd1234", "run-one")

    assert store.clear_baseline("archie", "abcd1234") is True
    assert store.load_baselines() == {}
    assert not store.baselines_path().exists()


def test_clearing_an_absent_baseline_reports_nothing_changed(benchmarks):
    assert store.clear_baseline("archie", "abcd1234") is False


def test_a_pinned_baseline_resolves_to_its_run(benchmarks, sample_run):
    store.save_run(sample_run)
    store.set_baseline(sample_run.host, sample_run.epoch, sample_run.run_id)

    assert store.baseline_run("archie", sample_run.epoch) == sample_run


def test_the_lock_excludes_a_second_holder(benchmarks):
    with store.exclusive(), pytest.raises(store.LockedError), store.exclusive():
        pass


def test_the_lock_is_released_after_use(benchmarks):
    with store.exclusive():
        pass

    with store.exclusive():
        pass


def test_a_lock_left_by_a_dead_process_is_taken_over(benchmarks):
    benchmarks.mkdir(parents=True, exist_ok=True)
    stale = benchmarks / store.LOCK
    stale.write_text("999999999\n", encoding="utf-8")

    with store.exclusive():
        assert stale.read_text(encoding="utf-8").strip() == str(os.getpid())


def test_a_lock_held_by_another_process_is_respected(benchmarks):
    benchmarks.mkdir(parents=True, exist_ok=True)
    path = benchmarks / store.LOCK
    holder = subprocess.Popen(
        [
            sys.executable,
            "-c",
            HOLD_LOCK,
            str(path),
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert holder.stdout.readline().strip() == "held"
        with pytest.raises(store.LockedError), store.exclusive():
            pass
    finally:
        holder.stdin.close()
        holder.wait(timeout=30)


def test_a_pid_file_without_a_live_lock_does_not_block(benchmarks):
    # The pid in the file names the holder; it is not what excludes anyone.
    # Treating it as authoritative is what let a recycled pid wedge the tool and
    # let a racing reader judge a freshly created lock stale and steal it.
    benchmarks.mkdir(parents=True, exist_ok=True)
    (benchmarks / store.LOCK).write_text(f"{os.getppid()}\n", encoding="utf-8")

    with store.exclusive():
        assert (benchmarks / store.LOCK).read_text(encoding="utf-8").strip() == str(os.getpid())


def test_pruning_keeps_the_newest_the_oldest_and_the_baseline(benchmarks):
    kept = []
    for day in range(1, 8):
        run = build_run(
            run_id=f"2026-08-0{day}T09-00-00Z-abcd1234",
            started=f"2026-08-0{day}T09:00:00Z",
            metrics=(metric("cpu.multi", [100.0]),),
        )
        store.save_run(run)
        kept.append(run)
    store.set_baseline("archie", kept[2].epoch, kept[2].run_id)

    dropped = {run.run_id for run in store.prunable(keep=2)}

    assert kept[-1].run_id not in dropped
    assert kept[0].run_id not in dropped
    assert kept[2].run_id not in dropped
    assert kept[3].run_id in dropped


def test_nothing_is_prunable_below_the_keep_count(benchmarks, sample_run):
    store.save_run(sample_run)

    assert store.prunable(keep=12) == []


def test_total_bytes_written_sums_the_runs(benchmarks):
    store.save_run(build_run(run_id="a", bytes_written=100))
    store.save_run(build_run(run_id="b", bytes_written=250))

    assert store.total_bytes_written("archie") == 350
