# Wrapper for coding agents
_run_agent() {
  command env \
    AGENT_SHELL=1 \
    "$@"
}

claude() {
  _run_agent claude --dangerously-skip-permissions "$@"
}

codex() {
  _run_agent codex --yolo "$@"
}

opencode-max() {
  _run_agent OMO_PROFILE=hybrid-max opencode "$@" --auto
}

opencode-light() {
  _run_agent OMO_PROFILE=hybrid-light opencode "$@" --auto
}

pi() {
  _run_agent pi "$@"
}

has_cmd opencode && alias opencode-stats="opencode stats --days 7 --models 10 --tools 20"
