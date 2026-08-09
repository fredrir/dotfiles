from dataclasses import dataclass, field


@dataclass
class Turn:
    kind: str
    title: str
    body: str


@dataclass
class Round:
    timestamp: object = None
    label: str = ""
    turns: list = field(default_factory=list)


@dataclass
class Session:
    provider: str
    session_id: str
    source_path: str
    cwd: str = ""
    model: str = ""
    title: str = ""
    started: object = None
    rounds: list = field(default_factory=list)
    degraded: bool = False
    raw_text: str = ""

    @property
    def user_rounds(self):
        return sum(1 for r in self.rounds if any(t.kind == "me" for t in r.turns))
