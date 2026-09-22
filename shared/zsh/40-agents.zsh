# Wrapper for coding agents
_run_agent() {
  command env \
    AGENT_SHELL=1 \
    "$@"
}

if has_cmd claude; then
  claude() {
    _run_agent claude --dangerously-skip-permissions "$@"
  }
fi

if has_cmd codex; then
  codex() {
    _run_agent codex --yolo "$@"
  }
  cod() {
    _run_agent codex --yolo "$@"
  }
  coda() {
    _run_agent codex --profile alibaba --yolo "$@"
  }
  codd() {
    _run_agent codex --profile deepseek --yolo "$@"
  }
fi

if has_cmd pi; then
  pi() {
    _run_agent pi "$@"
  }
fi

if has_cmd agy; then
  agy() {
    _run_agent agy --dangerously-skip-permissions "$@"
  }
fi
