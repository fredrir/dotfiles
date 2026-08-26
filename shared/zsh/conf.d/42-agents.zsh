# Wrapper for coding agents

_run_agent() {
  command env \
    -u PAGER \
    -u MANPAGER \
    -u LESS \
    -u MANWIDTH \
    -u EDITOR \
    -u VISUAL \
    -u GIT_PAGER \
    -u GIT_EDITOR \
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

