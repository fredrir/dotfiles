class RolloutError(Exception):
    """A release invariant failed without changing the requested state."""


class StateError(RolloutError):
    """The private rollout journal is missing, corrupt, or unsafe."""


class CommandError(RolloutError):
    """A bounded child command failed."""


class Refusal(RolloutError):
    """A live-state safety check refused a mutation."""
