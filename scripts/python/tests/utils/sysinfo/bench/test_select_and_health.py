from builders import build_run, metric, recent

from tools.utils.sysinfo.bench import select, store
from tools.utils.sysinfo.bench.health import benchmark_issues


def test_a_bare_host_parses_to_a_host_selector():
    assert select.parse("archie") == select.Selector(host="archie")


def test_every_dimension_can_be_pinned():
    selector = select.parse("archie/arch@a3f19c2e:2026-08-13T09-00-00Z-a3f19c2e")

    assert selector.host == "archie"
    assert selector.os_id == "arch"
    assert selector.epoch == "a3f19c2e"
    assert selector.run_id == "2026-08-13T09-00-00Z-a3f19c2e"


def test_a_selector_describes_itself_the_way_it_was_written():
    text = "archie/ubuntu@a3f19c2e"

    assert select.parse(text).describe() == text


def test_matching_filters_on_the_installation(benchmarks):
    run = build_run(install={"os": "arch"})

    assert select.matches(run, select.parse("archie/arch")) is True
    assert select.matches(run, select.parse("archie/ubuntu")) is False


def test_matching_filters_on_the_host(benchmarks):
    run = build_run(host="archie")

    assert select.matches(run, select.parse("macie")) is False


def test_resolving_prefers_the_newest_clean_run(benchmarks):
    older = build_run(run_id="older", started="2026-08-01T09:00:00Z")
    newer = build_run(run_id="newer", started="2026-08-09T09:00:00Z")
    store.save_run(older)
    store.save_run(newer)

    assert select.resolve(select.parse("archie")).run_id == "newer"


def test_resolving_falls_back_to_a_noisy_run_when_nothing_is_clean(benchmarks):
    store.save_run(build_run(run_id="only", grade="noisy"))

    assert select.resolve(select.parse("archie")).run_id == "only"


def test_resolving_an_unknown_host_finds_nothing(benchmarks):
    assert select.resolve(select.parse("nowhere")) is None


def test_a_machine_without_runs_has_no_findings(benchmarks):
    assert benchmark_issues("archie") == ()


def test_no_host_means_no_findings(benchmarks):
    assert benchmark_issues("") == ()


def test_a_run_without_a_baseline_produces_no_regression(benchmarks, sample_run):
    store.save_run(sample_run)

    assert benchmark_issues("archie") == ()


def test_a_drop_against_the_baseline_becomes_a_warning(benchmarks, sample_run, slower_run):
    store.save_run(sample_run)
    store.save_run(slower_run)
    store.set_baseline("archie", sample_run.epoch, sample_run.run_id)

    issues = benchmark_issues("archie")

    assert len(issues) == 1
    assert issues[0].severity == "warning"
    assert "cpu.multi" in issues[0].title
    assert "below its baseline" in issues[0].title


def test_a_change_within_the_noise_band_is_not_reported(benchmarks, sample_run):
    steady = build_run(
        run_id="2026-08-14T09-00-00Z-abcd1234",
        started=recent(0),
        metrics=(metric("cpu.multi", [100.0, 101.0, 99.0]),),
    )
    store.save_run(sample_run)
    store.save_run(steady)
    store.set_baseline("archie", sample_run.epoch, sample_run.run_id)

    assert benchmark_issues("archie") == ()


def test_a_stale_series_is_reported(benchmarks):
    store.save_run(build_run(run_id="ancient", started="2020-01-01T09:00:00Z"))

    issues = benchmark_issues("archie")

    assert any("stale" in issue.title for issue in issues)
