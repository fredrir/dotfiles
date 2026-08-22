# Wrapper for coding agents

claude() {
  AGENT_SHELL=1 command claude --dangerously-skip-permissions "$@"
}

codex() {
  AGENT_SHELL=1 command codex --yolo "$@"
}

opencode() {
  AGENT_SHELL=1 command opencode "$@"
}
