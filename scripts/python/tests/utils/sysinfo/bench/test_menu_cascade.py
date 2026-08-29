import pytest
from builders import build_run, metric

from tools.core.menu import Pick
from tools.utils.sysinfo.bench import cli

HOSTS = ["archie", "ubuntu"]


def make_run(run_id, host, keys=("cpu.multi",)):
    return build_run(
        run_id=run_id,
        host=host,
        metrics=tuple(metric(key, [100.0, 101.0]) for key in keys),
    )


RUNS = [make_run("aaa1", "archie"), make_run("bbb2", "ubuntu", ("cpu.multi", "disk.read"))]


@pytest.fixture
def store(monkeypatch):
    monkeypatch.setattr(cli.store, "known_hosts", lambda: list(HOSTS))
    monkeypatch.setattr(cli.store, "list_runs", lambda host=None, grades=None: list(RUNS))
    monkeypatch.setattr(cli.select, "epochs", lambda host: {"a3f1": RUNS, "b7c2": RUNS[:1]})
    monkeypatch.setattr(cli.select, "installs", lambda host: {"arch": RUNS, "ubuntu": RUNS[:1]})
    return cli._Runs()


def walk(expand, options):
    picks = ()
    for option in options:
        column = expand(picks)
        picks += (Pick(column.kind, column.options.index(option), option),)
    return picks


def opened(runs, options, flow=""):
    expand = cli._expand(runs, flow=flow)
    return expand(walk(expand, options))


def test_the_root_column_is_the_menu(store):
    column = cli._expand(store)(())
    assert column.kind == "menu"
    assert column.options == [name for name, _ in cli.MENU]


@pytest.mark.parametrize("option", ["run", "health", "list", "prune"])
def test_the_leaf_entries_open_nothing(store, option):
    assert opened(store, [option]) is None


def test_show_opens_the_run_column(store):
    column = opened(store, ["show"])
    assert (column.kind, column.title) == ("run", "show which run?")
    assert column.options == ["aaa1", "bbb2"]
    assert column.details == ["archie  arch  quick  clean", "ubuntu  arch  quick  clean"]


def test_compare_opens_the_comparison_column(store):
    column = opened(store, ["compare"])
    assert column.kind == "compare"
    assert column.options == [name for name, _ in cli.COMPARISONS]


def test_the_second_machine_column_drops_the_first_choice(store):
    column = opened(store, ["compare", cli.MACHINES, "archie"])
    assert (column.kind, column.title) == ("host-b", "second machine")
    assert column.options == ["ubuntu"]


def test_one_machine_is_not_a_comparison(store, monkeypatch):
    monkeypatch.setattr(cli.store, "known_hosts", lambda: ["archie"])
    column = opened(cli._Runs(), ["compare", cli.MACHINES])
    assert column.kind == "note"
    assert column.options == ["two machines are needed; only one has runs"]


def test_paired_columns_are_titled_so_they_can_be_told_apart(store):
    first = opened(store, ["compare", cli.UPGRADE, "archie"])
    assert (first.kind, first.title) == ("epoch-a", "earlier configuration")
    second = opened(store, ["compare", cli.UPGRADE, "archie", "a3f1"])
    assert (second.kind, second.title) == ("epoch-b", "later configuration")
    assert second.options == ["b7c2"]


def test_installs_are_paired_the_same_way(store):
    first = opened(store, ["compare", cli.DISTROS, "archie"])
    assert (first.kind, first.title) == ("install-a", "first installation")
    assert first.details == ["2 runs", "1 runs"]
    second = opened(store, ["compare", cli.DISTROS, "archie", "arch"])
    assert (second.kind, second.options) == ("install-b", ["ubuntu"])


def test_picking_two_runs_by_hand_uses_two_run_columns(store):
    assert opened(store, ["compare", cli.BY_HAND]).kind == "run-a"
    assert opened(store, ["compare", cli.BY_HAND, "aaa1"]).kind == "run-b"


def test_trend_asks_for_a_machine_then_a_metric(store):
    assert opened(store, ["trend"]).kind == "host"
    column = opened(store, ["trend", "ubuntu"])
    assert (column.kind, column.options) == ("metric", ["cpu.multi", "disk.read"])


def test_a_single_machine_skips_the_host_column(monkeypatch):
    monkeypatch.setattr(cli.store, "known_hosts", lambda: ["archie"])
    monkeypatch.setattr(cli.store, "list_runs", lambda host=None, grades=None: list(RUNS))
    column = opened(cli._Runs(), ["trend"])
    assert column.kind == "metric"


def test_a_machine_with_no_clean_runs_becomes_a_note(store, monkeypatch):
    monkeypatch.setattr(cli.store, "list_runs", lambda host=None, grades=None: [])
    column = opened(cli._Runs(), ["trend", "archie"])
    assert (column.kind, column.options) == ("note", ["archie has no clean runs"])


def test_baseline_asks_for_a_machine_then_a_run(store):
    column = opened(store, ["baseline", "archie"])
    assert (column.kind, column.title) == ("run", "use which run as the baseline?")


def test_a_flow_opens_inside_its_branch(store):
    assert cli._expand(store, flow="trend")(()).kind == "host"
    assert cli._expand(store, flow="show")(()).kind == "run"
    assert cli._expand(store, flow="compare")(()).kind == "compare"


def test_a_flow_still_skips_the_host_column(monkeypatch):
    monkeypatch.setattr(cli.store, "known_hosts", lambda: ["archie"])
    monkeypatch.setattr(cli.store, "list_runs", lambda host=None, grades=None: list(RUNS))
    assert cli._expand(cli._Runs(), flow="trend")(()).kind == "metric"


def test_the_host_is_recovered_when_its_column_was_skipped(store):
    assert cli._host_of((), ["archie"]) == "archie"
    picks = walk(cli._expand(store), ["trend", "ubuntu"])
    assert cli._host_of(picks, HOSTS) == "ubuntu"


def test_run_sides_come_from_the_cached_runs_not_a_reparse(store):
    picks = walk(cli._expand(store), ["compare", cli.BY_HAND, "aaa1", "bbb2"])
    assert cli._sides(store, picks) == (RUNS[0], RUNS[1])


def test_epoch_sides_resolve_through_the_group_map(store):
    picks = walk(cli._expand(store), ["compare", cli.UPGRADE, "archie", "a3f1", "b7c2"])
    assert cli._sides(store, picks) == (RUNS[0], RUNS[0])


def test_the_store_is_read_once_per_cascade(monkeypatch):
    reads = []
    monkeypatch.setattr(cli.store, "known_hosts", lambda: reads.append("hosts") or list(HOSTS))
    monkeypatch.setattr(
        cli.store, "list_runs", lambda host=None, grades=None: reads.append("runs") or list(RUNS)
    )
    runs = cli._Runs()
    expand = cli._expand(runs)
    for _attempt in range(3):
        expand(walk(expand, ["show"]))
        expand(walk(expand, ["trend"]))
    assert reads.count("hosts") == 1
    assert reads.count("runs") == 1
