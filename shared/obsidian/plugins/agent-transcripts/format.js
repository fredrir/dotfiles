const TURN_KINDS = {
  me: { title: "You", fold: "+" },
  turn: { title: "Response", fold: "-" },
  tool: { title: "Tool", fold: "-" },
};

function quoteLines(text, prefix) {
  return text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => (prefix + line).trimEnd())
    .join("\n");
}

function wrapAgentTranscript(text, provider, date) {
  const body = quoteLines(text.replace(/\s+$/, ""), "> ");
  return `> [!${provider}]- ${date}\n${body}\n`;
}

function markTranscriptTurn(text, kind) {
  const turn = TURN_KINDS[kind];
  if (!turn || !text) return text;
  const unquoted = text
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => line.replace(/^>\s?/, ""))
    .join("\n");
  const body = quoteLines(unquoted, "> > ");
  return `> > [!${kind}]${turn.fold} ${turn.title}\n${body}`;
}

function swapProviderText(text, from, to) {
  return text
    .replace(new RegExp(`(^[>\\s]*\\[!)${from}(\\]|\\|)`, "gm"), `$1${to}$2`)
    .replace(new RegExp(`(\\[!turn\\|)${from}(\\])`, "g"), `$1${to}$2`)
    .replace(/^provider:.*$/m, `provider: ${to}`);
}

function formatLocalDate(date) {
  const year = String(date.getFullYear()).padStart(4, "0");
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

module.exports = {
  formatLocalDate,
  markTranscriptTurn,
  swapProviderText,
  wrapAgentTranscript,
};
