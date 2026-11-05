import { cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { $ } from "bun";

const root = import.meta.dir;
const cargo = path.resolve(root, "../../../rust");
const packages = ["dotfmt", "dotfmt-lsp"];

export const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-musl",
  "win32-x64": "x86_64-pc-windows-gnu",
} as const;

export type Platform = keyof typeof TARGETS;

const DEFAULT_PLATFORMS: Platform[] = ["darwin-arm64", "linux-x64"];

export const isPlatform = (name: string): name is Platform => name in TARGETS;

const executable = (name: string, platform: Platform): string =>
  platform.startsWith("win32") ? `${name}.exe` : name;

async function checkBuildPrerequisites(targets: string[]): Promise<void> {
  const missing: string[] = [];
  if (!Bun.which("zig")) {
    missing.push("zig; install it from https://ziglang.org/download/");
  }
  if (!Bun.which("cargo-zigbuild")) {
    missing.push(
      "cargo-zigbuild; install it with `cargo install cargo-zigbuild`",
    );
  }
  if (Bun.which("rustup")) {
    const installed = await $`rustup target list --installed`.quiet().nothrow();
    if (installed.exitCode === 0) {
      const available = new Set(
        installed.stdout.toString().split(/\r?\n/).filter(Boolean),
      );
      for (const target of targets) {
        if (!available.has(target)) {
          missing.push(
            `Rust target ${target}; run \`rustup target add ${target}\``,
          );
        }
      }
    }
  }
  if (missing.length > 0) {
    throw new Error(`dotfmt cannot build:\n- ${missing.join("\n- ")}`);
  }
}

export async function build(
  platforms: Platform[] = DEFAULT_PLATFORMS,
): Promise<void> {
  const targetTriples = [
    ...new Set(platforms.map((platform) => TARGETS[platform])),
  ];
  const targets = targetTriples.flatMap((target) => ["--target", target]);
  const selected = packages.flatMap((name) => ["--package", name]);

  await checkBuildPrerequisites(targetTriples);
  await $`cargo zigbuild --locked --release ${selected} ${targets}`.cwd(cargo);

  await rm(path.join(root, "bin"), { recursive: true, force: true });
  for (const platform of platforms) {
    const directory = path.join(root, "bin", platform);
    await mkdir(directory, { recursive: true });
    for (const name of packages) {
      await cp(
        path.join(
          cargo,
          "target",
          TARGETS[platform],
          "release",
          executable(name, platform),
        ),
        path.join(directory, executable(name, platform)),
      );
    }
  }

  const bundle = await Bun.build({
    entrypoints: [path.join(root, "src/extension.ts")],
    outdir: path.join(root, "dist"),
    target: "node",
    format: "cjs",
    external: ["vscode"],
    minify: true,
  });
  if (!bundle.success) {
    throw new AggregateError(bundle.logs, "bundle failed");
  }
}

if (import.meta.main) {
  const { values } = parseArgs({
    options: {
      host: { type: "boolean" },
      target: { type: "string", multiple: true },
    },
  });
  const requested = (values.target ?? [])
    .flatMap((value) => value.split(","))
    .map((value) => value.trim())
    .filter(Boolean);
  const platforms: Platform[] = [];
  for (const name of requested) {
    if (!isPlatform(name)) {
      throw new Error(
        `unknown dotfmt target ${name}; expected ${Object.keys(TARGETS).join(
          ", ",
        )}`,
      );
    }
    platforms.push(name);
  }
  if (values.host && platforms.length > 0) {
    throw new Error("--host and --target cannot be combined");
  }
  if (values.host) {
    const host = `${process.platform}-${process.arch}`;
    if (!isPlatform(host)) {
      throw new Error(`no dotfmt target for ${host}`);
    }
    platforms.push(host);
  }
  await build(platforms.length > 0 ? [...new Set(platforms)] : undefined);
}
