<%*
const sel = tp.file.selection();
const pick = await tp.system.suggester(
  ["Claude", "Codex", "Agent"],
  ["claude", "codex", "agent"],
  false,
  "Wrap paste as…"
);
if (!pick) {
  tR += sel;
} else {
  const raw = sel && sel.trim().length ? sel : await tp.system.clipboard();
  const body = (raw ?? "")
    .replace(/\r\n/g, "\n")
    .replace(/\s+$/, "")
    .split("\n")
    .map((line) => ("> " + line).trimEnd())
    .join("\n");
  tR += `> [!${pick}]- ${tp.date.now("YYYY-MM-DD")}\n${body}\n`;
}
%>
