import path from "node:path";
import { describe, expect, test } from "bun:test";
import {
  artifactPlatforms,
  hostTarget,
  localInstallDecision,
  missingArtifacts,
  requiredArtifacts,
  staleArtifacts,
} from "./deploy-lib.ts";

describe("hostTarget", () => {
  test("maps the supported Node platform and arch pairs", () => {
    expect(hostTarget("darwin", "arm64")).toBe("darwin-arm64");
    expect(hostTarget("linux", "x64")).toBe("linux-x64");
    expect(hostTarget("win32", "x64")).toBe("win32-x64");
  });

  test("returns undefined for hosts dotfmt does not build for", () => {
    expect(hostTarget("linux", "arm64")).toBeUndefined();
    expect(hostTarget("freebsd", "x64")).toBeUndefined();
  });
});

describe("localInstallDecision", () => {
  test("allows a universal vsix and a matching target", () => {
    expect(localInstallDecision(undefined, "darwin-arm64").install).toBe(true);
    expect(localInstallDecision("darwin-arm64", "darwin-arm64").install).toBe(
      true,
    );
  });

  test("skips a foreign platform instead of installing it locally", () => {
    const decision = localInstallDecision("linux-x64", "darwin-arm64");
    expect(decision.install).toBe(false);
    expect(decision.reason).toContain("linux-x64");
    expect(decision.reason).toContain("darwin-arm64");
  });

  test("skips a target the host cannot run or identify", () => {
    expect(localInstallDecision("linux-x64", undefined).install).toBe(false);
  });

  test("does not install the default package on a host it does not contain", () => {
    const decision = localInstallDecision(undefined, "win32-x64");
    expect(decision.install).toBe(false);
    expect(decision.reason).toContain("win32-x64");
    expect(localInstallDecision(undefined, "linux-x64").install).toBe(true);
  });

  test("uses the packaged platform set when deciding", () => {
    expect(
      localInstallDecision(undefined, "linux-x64", ["linux-x64"]).install,
    ).toBe(true);
    expect(
      localInstallDecision(undefined, "darwin-arm64", ["linux-x64"]).install,
    ).toBe(false);
  });
});

describe("artifacts", () => {
  const root = "/app";
  const darwin = requiredArtifacts(root, ["darwin-arm64"]);
  const windows = requiredArtifacts(root, ["win32-x64"]);

  test("requires the bundle and both bundled binaries", () => {
    expect(darwin).toEqual([
      path.join(root, "dist/extension.js"),
      path.join(root, "bin/darwin-arm64/dotfmt-lsp"),
      path.join(root, "bin/darwin-arm64/dotfmt"),
    ]);
    expect(windows).toContain(path.join(root, "bin/win32-x64/dotfmt-lsp.exe"));
  });

  test("packages the requested platforms, else the default build", () => {
    expect(artifactPlatforms(["linux-x64"])).toEqual(["linux-x64"]);
    expect(artifactPlatforms([])).toContain("darwin-arm64");
  });

  test("reports artifacts older than the newest source", () => {
    const mtime = (file: string) =>
      file.endsWith("extension.js") ? 100 : 50;
    expect(staleArtifacts(root, ["darwin-arm64"], 75, mtime)).toEqual([
      path.join(root, "bin/darwin-arm64/dotfmt-lsp"),
      path.join(root, "bin/darwin-arm64/dotfmt"),
    ]);
    expect(staleArtifacts(root, ["darwin-arm64"], 10, mtime)).toEqual([]);
  });

  test("reports only the missing artifacts", () => {
    const present = new Set([path.join(root, "dist/extension.js")]);
    expect(
      missingArtifacts(root, ["darwin-arm64"], (file) => present.has(file)),
    ).toEqual([
      path.join(root, "bin/darwin-arm64/dotfmt-lsp"),
      path.join(root, "bin/darwin-arm64/dotfmt"),
    ]);
  });
});
