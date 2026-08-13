from builders import SNAPSHOT, build_run, metric

from tools.utils.sysinfo.bench.record import (
    HIB,
    LIB,
    Metric,
    Run,
    comparable_methods,
    epoch_of,
    method_series,
    relative_deviation,
    snapshot_differences,
)


def test_a_metric_summarises_its_samples():
    entry = metric("cpu.multi", [100.0, 102.0, 104.0])

    assert entry.times_to_run == 3
    assert entry.median == 102.0
    assert entry.mad == 2.0


def test_a_single_sample_has_no_spread():
    entry = metric("cpu.multi", [100.0])

    assert entry.median == 100.0
    assert entry.mad == 0.0
    assert entry.rsd_pct == 0.0


def test_a_metric_without_samples_reports_nothing():
    entry = metric("cpu.multi", [])

    assert entry.median is None
    assert entry.times_to_run == 0


def test_relative_deviation_is_a_percentage():
    assert relative_deviation([100.0, 100.0, 100.0]) == 0.0
    assert relative_deviation([90.0, 100.0, 110.0]) > 9.0


def test_higher_is_better_and_lower_is_better_disagree():
    higher = metric("cpu.multi", [100.0], proportion=HIB)
    lower = metric("workload.nvim", [100.0], proportion=LIB)

    assert higher.better(90.0) is True
    assert lower.better(90.0) is False


def test_the_epoch_follows_the_identity_bearing_fields():
    changed = dict(SNAPSHOT)
    changed["gpu"] = [{"name": "NVIDIA GeForce RTX 3080", "memory_total": 10737418240}]

    assert epoch_of(SNAPSHOT) != epoch_of(changed)
    assert len(epoch_of(SNAPSHOT)) == 8


def test_the_epoch_ignores_fields_that_do_not_identify_hardware():
    same = dict(SNAPSHOT)
    same["virtualized"] = True
    same["configured"] = {"case": "a different case"}

    assert epoch_of(same) == epoch_of(SNAPSHOT)


def test_a_method_series_drops_the_patch_level():
    assert method_series("cpu.multi/1.2.3") == "cpu.multi/1.2"


def test_a_patch_bump_stays_comparable_but_a_minor_bump_does_not():
    base = metric("cpu.multi", [1.0], method="cpu.multi/1.0.0")
    patched = metric("cpu.multi", [1.0], method="cpu.multi/1.0.1")
    revised = metric("cpu.multi", [1.0], method="cpu.multi/1.1.0")

    assert comparable_methods(base, patched) is True
    assert comparable_methods(base, revised) is False


def test_a_swapped_gpu_shows_up_as_a_difference():
    after = dict(SNAPSHOT)
    after["gpu"] = [{"name": "NVIDIA GeForce RTX 5070 Ti", "memory_total": 17094934528}]
    before = dict(SNAPSHOT)
    before["gpu"] = [{"name": "NVIDIA GeForce RTX 3080", "memory_total": 10737418240}]

    changes = snapshot_differences(before, after)

    assert ("GPU", "name", "NVIDIA GeForce RTX 3080", "NVIDIA GeForce RTX 5070 Ti") in changes


def test_identical_snapshots_differ_in_nothing():
    assert snapshot_differences(SNAPSHOT, dict(SNAPSHOT)) == ()


def test_a_run_survives_a_json_round_trip():
    original = build_run(
        metrics=(metric("cpu.multi", [100.0, 101.0]),),
        note="after repasting",
        tags=("thermal",),
        dotfiles_sha="eea48be",
        bytes_written=2048,
    )

    restored = Run.from_json(original.to_json())

    assert restored == original
    assert restored.epoch == original.epoch
    assert restored.metric("cpu.multi").samples == (100.0, 101.0)


def test_the_serialised_run_carries_its_derived_epoch():
    payload = build_run().to_json()

    assert payload["epoch"] == epoch_of(SNAPSHOT)


def test_an_unknown_metric_is_absent_rather_than_an_error():
    assert build_run().metric("nothing.here") is None


def test_a_metric_reports_its_family():
    assert Metric("workload.git_status", "", "", HIB, "").family == "workload"
