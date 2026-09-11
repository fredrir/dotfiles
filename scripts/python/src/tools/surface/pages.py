"""Which tools each page in `docs/cli` documents, and where they live."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Page:
    name: str  # the file stem under docs/cli
    title: str
    programs: tuple
    source: str


from tools.surface.catalog import CATALOG

PAGES = tuple(Page(**(page | {"programs": tuple(page["programs"])})) for page in CATALOG["pages"])
UNDOCUMENTED = tuple(CATALOG["undocumented"])
RUST = tuple(CATALOG["native"])


def page_for(program):
    for page in PAGES:
        if program in page.programs:
            return page
    return None


def dispatched(program):
    """The subcommands `program` gains from a `program-<name>` binary on PATH."""
    found = []
    for page in PAGES:
        for binary in page.programs:
            head, _, name = binary.partition("-")
            if head == program and name and page.title == f"{program} {name}":
                found.append((name, binary))
    return tuple(found)
