import { describe, expect, test } from "bun:test";
import { formatTextParams } from "./paste";

describe("formatTextParams", () => {
  test("sends the language id for languages the server maps", () => {
    expect(formatTextParams("a=1\n", "ini", "file:///work/a.ini")).toEqual({
      text: "a=1\n",
      languageId: "ini",
      uri: "file:///work/a.ini",
    });
  });

  test("omits plaintext so the server resolves glob-only files by name", () => {
    expect(
      formatTextParams("<plist/>", "plaintext", "file:///work/a.plist"),
    ).toEqual({ text: "<plist/>", uri: "file:///work/a.plist" });
  });

  test("skips when neither a known language nor a file URI is available", () => {
    expect(
      formatTextParams("@book{x}", "plaintext", "untitled:Untitled-1"),
    ).toBeUndefined();
  });
});
