import path from "node:path";
import { describe, expect, test } from "bun:test";
import { findConfig, isWithin } from "./paths";

describe("isWithin", () => {
  test("accepts the root and its children", () => {
    expect(isWithin("/workspace/project", "/workspace/project")).toBe(true);
    expect(isWithin("/workspace/project", "/workspace/project/src")).toBe(true);
    expect(
      isWithin("/workspace/project", "/workspace/project/src/app.ts"),
    ).toBe(true);
  });

  test("rejects sibling paths with the same prefix", () => {
    expect(isWithin("/workspace/project", "/workspace/project-other")).toBe(
      false,
    );
    expect(
      isWithin("/workspace/project", "/workspace/project-other/app.ts"),
    ).toBe(false);
    expect(isWithin("/workspace/project", "/workspace")).toBe(false);
  });

  test("accepts child names that begin with dots", () => {
    expect(
      isWithin("/workspace/project", "/workspace/project/..notes/a.ts"),
    ).toBe(true);
  });
});

describe("findConfig", () => {
  test("finds the nearest config and stops at the workspace root", () => {
    const root = path.resolve("/workspace/project");
    const file = path.join(root, "src/deep/app.ts");
    const nearest = path.join(root, "src", "dotfmt.dotfile");
    const found = findConfig(file, root, (candidate) => candidate === nearest);
    expect(found).toBe(nearest);
  });

  test("never walks above the workspace root", () => {
    const root = path.resolve("/workspace/project");
    const file = path.join(root, "src/app.ts");
    const outside = path.resolve("/dotfmt.dotfile");
    const found = findConfig(file, root, (candidate) => candidate === outside);
    expect(found).toBeUndefined();
  });
});
