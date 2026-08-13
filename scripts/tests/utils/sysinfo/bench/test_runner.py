from tools.utils.sysinfo.bench import runner
from tools.utils.sysinfo.bench.limits import MAX_RUNS, MIN_RUNS
from tools.utils.sysinfo.bench.record import HIB, WORLD
from tools.utils.sysinfo.bench.suites import Job, MeasurementError, Output


def build_job(values, repeat=True, outputs=None):
    calls = {"count": 0}

    def measure():
        index = calls["count"]
        calls["count"] += 1
        return values[min(index, len(values) - 1)]

    job = Job(
        name="test",
        tool="fake",
        version="1.0",
        method="test/1.0.0",
        outputs=outputs or (Output("test.value", "units", HIB, WORLD),),
        measure=measure,
        repeat=repeat,
    )
    return job, calls


def test_a_steady_metric_stops_at_the_minimum_run_count():
    job, calls = build_job([{"test.value": 100.0}])

    collected = runner.measure_job(job)

    assert calls["count"] == MIN_RUNS
    assert len(collected["test.value"]) == MIN_RUNS


def test_a_noisy_metric_runs_up_to_the_ceiling():
    job, calls = build_job(
        [
            {"test.value": 50.0},
            {"test.value": 150.0},
            {"test.value": 60.0},
            {"test.value": 140.0},
            {"test.value": 70.0},
            {"test.value": 130.0},
        ]
    )

    collected = runner.measure_job(job)

    assert calls["count"] == MAX_RUNS
    assert len(collected["test.value"]) == MAX_RUNS


def test_a_job_that_does_not_repeat_is_measured_once():
    job, calls = build_job([{"test.value": 100.0}], repeat=False)

    runner.measure_job(job)

    assert calls["count"] == 1


def test_convergence_needs_every_output_to_settle():
    outputs = (
        Output("test.value", "units", HIB, WORLD),
        Output("test.other", "units", HIB, WORLD),
    )
    job, calls = build_job(
        [
            {"test.value": 100.0, "test.other": 10.0},
            {"test.value": 100.0, "test.other": 90.0},
            {"test.value": 100.0, "test.other": 20.0},
            {"test.value": 100.0, "test.other": 80.0},
            {"test.value": 100.0, "test.other": 30.0},
            {"test.value": 100.0, "test.other": 70.0},
        ],
        outputs=outputs,
    )

    runner.measure_job(job)

    assert calls["count"] == MAX_RUNS


def test_metrics_are_built_only_for_outputs_that_produced_samples():
    outputs = (
        Output("test.value", "units", HIB, WORLD),
        Output("test.missing", "units", HIB, WORLD),
    )
    job, _calls = build_job([{"test.value": 100.0}], outputs=outputs)

    metrics = runner.metrics_for(job, runner.measure_job(job))

    assert [metric.key for metric in metrics] == ["test.value"]
    assert metrics[0].tool == "fake"
    assert metrics[0].method == "test/1.0.0"


def test_an_empty_collection_never_counts_as_converged():
    assert runner.converged({}) is False


def test_a_family_filter_keeps_only_matching_jobs(monkeypatch):
    cpu_job, _ = build_job([{"cpu.value": 1.0}], outputs=(Output("cpu.value", "u", HIB, WORLD),))
    disk_job, _ = build_job([{"disk.value": 1.0}], outputs=(Output("disk.value", "u", HIB, WORLD),))
    monkeypatch.setattr(runner, "SUITES", ())
    setting = runner.Setting(tier="quick", workdir="/tmp", families=("cpu",))

    kept = [job for job in (cpu_job, disk_job) if runner.family_of(job) in setting.families]

    assert kept == [cpu_job]


def test_a_measurement_error_names_the_family_it_came_from():
    job, _calls = build_job([{"test.value": 100.0}])

    assert runner.family_of(job) == "test"
    assert issubclass(MeasurementError, Exception)
