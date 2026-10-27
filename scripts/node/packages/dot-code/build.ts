import { $ } from "bun";
import { cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";

const root = import.meta.dir;
const cargo = path.resolve(root, "../../../rust");
const server = "dot-code-server";

/** VS Code `<platform>-<arch>` mapped to the Rust target its server is built for. */
export const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-musl",
} as const;

export type Platform = keyof typeof TARGETS;

const isPlatform = (name: string): name is Platform => name in TARGETS;

export async function build(
  platforms: Platform[] = Object.keys(TARGETS).filter(isPlatform),
): Promise<void> {
  const targets = platforms.flatMap((platform) => ["--target", TARGETS[platform]]);
  // rust-lld links static musl binaries without a cross toolchain.
  await $`cargo build --locked --release --package ${server} ${targets}`
    .cwd(cargo)
    .env({ ...process.env, CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER: "rust-lld" });

  await rm(path.join(root, "bin"), { recursive: true, force: true });
  for (const platform of platforms) {
    const directory = path.join(root, "bin", platform);
    await mkdir(directory, { recursive: true });
    await cp(
      path.join(cargo, "target", TARGETS[platform], "release", server),
      path.join(directory, server),
    );
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
  const { values } = parseArgs({ options: { host: { type: "boolean" } } });
  const host = `${process.platform}-${process.arch}`;
  if (values.host && !isPlatform(host)) {
    throw new Error(`no server target for ${host}`);
  }
  await build(values.host && isPlatform(host) ? [host] : undefined);
}
