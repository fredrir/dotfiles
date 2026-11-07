import { describe, expect, test } from "bun:test";
import { enabledFormatterLanguages } from "./languages";

describe("formatter language selection", () => {
  test("maps dotfmt names to installed VS Code language ids", () => {
    const selected = enabledFormatterLanguages(
      ["bib", "conf", "css", "typescript"],
      [
        "bibtex",
        "dotfile",
        "properties",
        "css",
        "scss",
        "typescript",
        "typescriptreact",
      ],
    );
    expect(selected).toEqual([
      "bibtex",
      "css",
      "dotfile",
      "properties",
      "scss",
      "typescript",
      "typescriptreact",
    ]);
  });

  test("never drops an enabled language the shipped list does not know", () => {
    expect(
      enabledFormatterLanguages(["toml", "json5"], ["toml", "json5"]),
    ).toEqual(["toml", "json5"]);
  });

  test("does not select installed languages that dotfmt did not enable", () => {
    expect(enabledFormatterLanguages(["toml"], ["toml", "yaml"])).toEqual([
      "toml",
    ]);
  });

  test("keeps plist enabled through its plist and xml language ids", () => {
    expect(enabledFormatterLanguages(["plist"], ["plist", "xml"])).toEqual([
      "plist",
      "xml",
    ]);
  });
});
