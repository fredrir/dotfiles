import { describe, expect, test } from "bun:test";
import { configTarget, formatSettingsQuery } from "./native";

describe("formatSettingsQuery", () => {
  test("filters Settings to the extension and generated format keys", () => {
    expect(formatSettingsQuery()).toBe("@ext:fredrir.dotfmt dotfmt.format");
  });
});

describe("configTarget", () => {
  const global = "/home/user/.config/dotfmt/dotfmt.dotfile";
  const local = "/workspace/dotfmt.dotfile";

  test("opens an existing local file", () => {
    expect(configTarget(local, global, (file) => file === local)).toBe(local);
  });

  test("opens an existing global file before prompting for a local one", () => {
    expect(configTarget(local, global, (file) => file === global)).toBe(global);
    expect(configTarget(undefined, global, (file) => file === global)).toBe(
      global,
    );
  });

  test("returns the local path to create when neither file exists", () => {
    expect(configTarget(local, global, () => false)).toBe(local);
    expect(configTarget(undefined, global, () => false)).toBe(global);
  });
});
