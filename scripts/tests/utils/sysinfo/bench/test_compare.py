from builders import SNAPSHOT, build_run, metric

from tools.utils.sysinfo.bench.compare import (
    BETTER,
    BLOCKED,
    NOISE,
    WORSE,
    compare_runs,
    noise_band,
    regressions,
)
from tools.utils.sysinfo.bench.record import HOST, LIB, PLATFORM


def compare_one(left_metric, right_metric, left_run=None, right_run=None):
    left = left_run or build_run(metrics=(left_metric,))
    right = right_run or build_run(run_id="right", metrics=(right_metric,))
    deltas, _changes, _only_left, _only_right = compare_runs(left, right)
    return deltas[0]


def test_a_small_change_is_reported_as_noise():
    delta = compare_one(metric("cpu.multi", [100.0]), metric("cpu.multi", [101.0]))

    assert delta.verdict == NOISE
    assert abs(delta.change_pct - 1.0) < 0.001


def test_a_large_gain_is_better_for_a_higher_is_better_metric():
    delta = compare_one(metric("cpu.multi", [100.0]), metric("cpu.multi", [150.0]))

    assert delta.verdict == BETTER
    assert delta.change_pct == 50.0


def test_a_large_loss_is_worse_for_a_higher_is_better_metric():
    delta = compare_one(metric("cpu.multi", [100.0]), metric("cpu.multi", [50.0]))

    assert delta.verdict == WORSE


def test_the_direction_inverts_for_a_lower_is_better_metric():
    faster = compare_one(
        metric("workload.nvim", [100.0], proportion=LIB),
        metric("workload.nvim", [50.0], proportion=LIB),
    )
    slower = compare_one(
        metric("workload.nvim", [100.0], proportion=LIB),
        metric("workload.nvim", [200.0], proportion=LIB),
    )

    assert faster.verdict == BETTER
    assert slower.verdict == WORSE


def test_a_noisy_metric_widens_its_own_band():
    steady = noise_band(metric("cpu.multi", [100.0, 100.0]), metric("cpu.multi", [100.0, 100.0]))
    jumpy = noise_band(metric("cpu.multi", [70.0, 130.0]), metric("cpu.multi", [70.0, 130.0]))

    assert jumpy > steady


def test_a_change_inside_a_wide_band_stays_noise():
    delta = compare_one(
        metric("cpu.multi", [70.0, 100.0, 130.0]),
        metric("cpu.multi", [75.0, 105.0, 135.0]),
    )

    assert delta.verdict == NOISE


def test_a_changed_method_blocks_the_comparison():
    delta = compare_one(
        metric("cpu.multi", [100.0], method="cpu.multi/1.0.0"),
        metric("cpu.multi", [150.0], method="cpu.multi/2.0.0"),
    )

    assert delta.verdict == BLOCKED
    assert "method changed" in delta.reason


def test_a_host_scoped_metric_does_not_cross_machines():
    left = build_run(host="archie", metrics=(metric("gpu.graphics", [100.0], comparable=HOST),))
    right = build_run(
        run_id="right", host="macie", metrics=(metric("gpu.graphics", [200.0], comparable=HOST),)
    )

    delta = compare_one(None, None, left, right)

    assert delta.verdict == BLOCKED
    assert "only comparable within one machine" in delta.reason


def test_a_platform_scoped_metric_does_not_cross_operating_systems():
    left = build_run(metrics=(metric("disk.seq_read", [100.0], comparable=PLATFORM),))
    right = build_run(
        run_id="right",
        install={"os": "ubuntu"},
        metrics=(metric("disk.seq_read", [200.0], comparable=PLATFORM),),
    )

    delta = compare_one(None, None, left, right)

    assert delta.verdict == BLOCKED
    assert "only comparable within one platform" in delta.reason


def test_a_world_scoped_metric_needs_the_same_tool_version():
    delta = compare_one(
        metric("cpu.multi", [100.0], version="26.02"),
        metric("cpu.multi", [150.0], version="24.09"),
    )

    assert delta.verdict == BLOCKED
    assert "different 7z version" in delta.reason


def test_a_workload_is_not_compared_across_configurations():
    left = build_run(
        dotfiles_sha="aaa1111", metrics=(metric("workload.nvim", [100.0], proportion=LIB),)
    )
    right = build_run(
        run_id="right",
        dotfiles_sha="bbb2222",
        metrics=(metric("workload.nvim", [200.0], proportion=LIB),),
    )

    delta = compare_one(None, None, left, right)

    assert delta.verdict == BLOCKED
    assert "configuration changed" in delta.reason


def test_a_workload_is_not_compared_against_an_uncommitted_tree():
    left = build_run(
        dotfiles_sha="aaa1111-dirty", metrics=(metric("workload.nvim", [100.0], proportion=LIB),)
    )
    right = build_run(
        run_id="right",
        dotfiles_sha="aaa1111-dirty",
        metrics=(metric("workload.nvim", [200.0], proportion=LIB),),
    )

    delta = compare_one(None, None, left, right)

    assert delta.verdict == BLOCKED
    assert "uncommitted working tree" in delta.reason


def test_a_workload_is_compared_when_the_configuration_matches():
    left = build_run(
        dotfiles_sha="aaa1111", metrics=(metric("workload.nvim", [100.0], proportion=LIB),)
    )
    right = build_run(
        run_id="right",
        dotfiles_sha="aaa1111",
        metrics=(metric("workload.nvim", [200.0], proportion=LIB),),
    )

    assert compare_one(None, None, left, right).verdict == WORSE


def test_a_hardware_metric_ignores_the_configuration():
    left = build_run(dotfiles_sha="aaa1111-dirty", metrics=(metric("cpu.multi", [100.0]),))
    right = build_run(
        run_id="right", dotfiles_sha="bbb2222", metrics=(metric("cpu.multi", [50.0]),)
    )

    assert compare_one(None, None, left, right).verdict == WORSE


def test_metrics_present_on_only_one_side_are_reported_separately():
    left = build_run(metrics=(metric("cpu.multi", [100.0]), metric("mem.read", [10.0])))
    right = build_run(run_id="right", metrics=(metric("cpu.multi", [100.0]),))

    deltas, _changes, only_left, only_right = compare_runs(left, right)

    assert [delta.key for delta in deltas] == ["cpu.multi"]
    assert only_left == ("mem.read",)
    assert only_right == ()


def test_a_hardware_change_is_surfaced_with_the_comparison():
    before = dict(SNAPSHOT)
    before["gpu"] = [{"name": "NVIDIA GeForce RTX 3080"}]
    left = build_run(snapshot=before, metrics=(metric("cpu.multi", [100.0]),))
    right = build_run(run_id="right", metrics=(metric("cpu.multi", [100.0]),))

    _deltas, changes, _only_left, _only_right = compare_runs(left, right)

    assert any(change[0] == "GPU" for change in changes)


def test_only_large_losses_count_as_regressions():
    small = compare_one(metric("cpu.multi", [100.0]), metric("cpu.multi", [95.0]))
    large = compare_one(metric("cpu.multi", [100.0]), metric("cpu.multi", [50.0]))

    assert regressions([small], 10.0) == ()
    assert regressions([large], 10.0) == (large,)
