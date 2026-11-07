import { describe, expect, test } from "bun:test";
import { traceLevel } from "./trace";

describe("traceLevel", () => {
  test("maps the configured trace values", () => {
    expect(traceLevel("off")).toBe("off");
    expect(traceLevel("messages")).toBe("messages");
    expect(traceLevel("verbose")).toBe("verbose");
  });

  test("treats missing and unknown values as off", () => {
    expect(traceLevel(undefined)).toBe("off");
    expect(traceLevel("all")).toBe("off");
  });
});
