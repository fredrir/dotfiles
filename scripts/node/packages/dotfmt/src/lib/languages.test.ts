import { describe, expect, test } from "bun:test";

import {
  enabledFormatterLanguages,
  fallbackFormatterLanguages,
} from "./languages";

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

  test("keeps JavaScript out of the static fallback", () => {
    expect(
      fallbackFormatterLanguages([
        "typescript",
        "javascript",
        "javascriptreact",
      ]),
    ).toEqual(["typescript"]);
  });
});
