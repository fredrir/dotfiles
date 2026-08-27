from dataclasses import dataclass

from tools.utils.sysinfo.bench import store

CLEAN = ("clean",)
ANY = ("clean", "noisy", "aborted")


@dataclass(frozen=True)
class Selector:
    host: str = ""
    os_id: str = ""
    epoch: str = ""
    run_id: str = ""

    def describe(self):
        text = self.host
        if self.os_id:
            text += f"/{self.os_id}"
        if self.epoch:
            text += f"@{self.epoch}"
        if self.run_id:
            text += f":{self.run_id}"
        return text


def parse(text):
    rest, _, run_id = text.strip().partition(":")
    rest, _, epoch = rest.partition("@")
    host, _, os_id = rest.partition("/")
    return Selector(
        host=host.strip(), os_id=os_id.strip(), epoch=epoch.strip(), run_id=run_id.strip()
    )


def matches(run, selector):
    if selector.host and run.host != selector.host:
        return False
    if selector.os_id and run.install.get("os", "") != selector.os_id:
        return False
    if selector.epoch and run.epoch != selector.epoch:
        return False
    return not (selector.run_id and run.run_id != selector.run_id)


def candidates(selector, grades=CLEAN):
    runs = store.list_runs(selector.host or None, grades=grades)
    return [run for run in runs if matches(run, selector)]


def resolve(selector, grades=CLEAN):
    found = candidates(selector, grades)
    if found:
        return found[0]
    relaxed = candidates(selector, ANY)
    return relaxed[0] if relaxed else None


def epochs(host):
    found = {}
    for run in store.list_runs(host, grades=ANY):
        found.setdefault(run.epoch, []).append(run)
    return found


def installs(host):
    found = {}
    for run in store.list_runs(host, grades=ANY):
        found.setdefault(run.install.get("os", "unknown"), []).append(run)
    return found
