<%*
const sel = tp.file.selection();
const pick = await tp.system.suggester(
  ["Me — user message", "Turn — assistant reply", "Tool — command / output"],
  ["me", "turn", "tool"],
  false,
  "Mark selection as…"
);
if (!pick || !sel) {
  tR += sel ?? "";
} else {
  const titles = { me: "You", turn: "Response", tool: "Tool" };
  const fold = pick === "me" ? "" : "-";
  const body = sel
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => line.replace(/^>\s?/, ""))
    .map((line) => ("> > " + line).trimEnd())
    .join("\n");
  tR += `> > [!${pick}]${fold} ${titles[pick]}\n${body}`;
}
%>
