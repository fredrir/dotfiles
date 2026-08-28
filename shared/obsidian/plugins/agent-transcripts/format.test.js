const test = require("node:test");
const assert = require("node:assert/strict");

const {
	formatLocalDate,
	markTranscriptTurn,
	swapProviderText,
	wrapAgentTranscript,
} = require("./format");

test("wraps multiline text in a provider callout", () => {
	assert.equal(
		wrapAgentTranscript("hello\r\n\r\nworld  \n", "codex", "2026-08-13"),
		"> [!codex]- 2026-08-13\n> hello\n>\n> world\n",
	);
});

test("marks user, assistant, and tool turns", () => {
	assert.equal(
		markTranscriptTurn("hello\n> world", "me"),
		"> > [!me]+ You\n> > hello\n> > world",
	);
	assert.equal(
		markTranscriptTurn("done", "turn"),
		"> > [!turn]- Response\n> > done",
	);
	assert.equal(
		markTranscriptTurn("$ pwd", "tool"),
		"> > [!tool]- Tool\n> > $ pwd",
	);
});

test("leaves text unchanged for an unknown turn kind", () => {
	assert.equal(markTranscriptTurn("hello", "unknown"), "hello");
});

test("reassigns provider frontmatter and callouts", () => {
	const source = [
		"provider: claude",
		"> [!claude]- capture",
		"> > [!turn|claude]- Claude",
		"> body",
	].join("\n");
	const expected = [
		"provider: codex",
		"> [!codex]- capture",
		"> > [!turn|codex]- Claude",
		"> body",
	].join("\n");
	assert.equal(swapProviderText(source, "claude", "codex"), expected);
});

test("formats a local calendar date without using UTC conversion", () => {
	assert.equal(formatLocalDate(new Date(2026, 7, 3, 12)), "2026-08-03");
});
