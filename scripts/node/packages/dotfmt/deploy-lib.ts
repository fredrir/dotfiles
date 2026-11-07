import path from "node:path";
import { DEFAULT_PLATFORMS, isPlatform, type Platform } from "./build.ts";

export function hostTarget(
  platform: string = process.platform,
  arch: string = process.arch,
): Platform | undefined {
  const name = `${platform}-${arch}`;
  return isPlatform(name) ? name : undefined;
}

export function localInstallDecision(
  requested: Platform | undefined,
  host: Platform | undefined,
  packaged: readonly Platform[] = artifactPlatforms(
    requested === undefined ? [] : [requested],
  ),
): { install: boolean; reason?: string } {
  if (host === undefined) {
    return {
      install: false,
      reason:
        "skipping the local install: this host has no dotfmt target. Install the vsix on a supported platform, or use --remote <host>.",
    };
  }
  if (packaged.includes(host)) {
    return { install: true };
  }
  return {
    install: false,
    reason: `skipping the local install: the vsix contains ${packaged.join(
      ", ",
    )} but this host is ${host}. Build with --host/--target ${host}, install it on a packaged platform, or use --remote <host>.`,
  };
}

export function artifactPlatforms(
  requested: readonly Platform[],
): readonly Platform[] {
  return requested.length > 0 ? requested : DEFAULT_PLATFORMS;
}

export function requiredArtifacts(
  root: string,
  platforms: readonly Platform[],
): string[] {
  const files = [path.join(root, "dist", "extension.js")];
  for (const platform of platforms) {
    for (const name of ["dotfmt-lsp", "dotfmt"]) {
      files.push(
        path.join(
          root,
          "bin",
          platform,
          platform.startsWith("win32") ? `${name}.exe` : name,
        ),
      );
    }
  }
  return files;
}

export function missingArtifacts(
  root: string,
  platforms: readonly Platform[],
  exists: (file: string) => boolean,
): string[] {
  return requiredArtifacts(root, platforms).filter((file) => !exists(file));
}

export function staleArtifacts(
  root: string,
  platforms: readonly Platform[],
  newestSource: number,
  mtime: (file: string) => number,
): string[] {
  return requiredArtifacts(root, platforms).filter(
    (file) => mtime(file) < newestSource,
  );
}
