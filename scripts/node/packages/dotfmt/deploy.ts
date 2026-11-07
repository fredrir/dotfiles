import { existsSync, statSync } from "node:fs";
import { readdir, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";
import { $ } from "bun";
import { z } from "zod";
import { build, isPlatform, TARGETS, type Platform } from "./build.ts";
import {
  artifactPlatforms,
  hostTarget,
  localInstallDecision,
  missingArtifacts,
  staleArtifacts,
} from "./deploy-lib.ts";

const { values } = parseArgs({
  options: {
    remote: { type: "string" },
    host: { type: "boolean" },
    "skip-build": { type: "boolean" },
    target: { type: "string", multiple: true },
  },
});

const root = import.meta.dir;

const requested = (values.target ?? [])
  .flatMap((value) => value.split(","))
  .map((value) => value.trim())
  .filter(Boolean);
const targets: Platform[] = [];
for (const name of requested) {
  if (!isPlatform(name)) {
    throw new Error(
      `unknown dotfmt target ${name}; expected ${Object.keys(TARGETS).join(
        ", ",
      )}`,
    );
  }
  targets.push(name);
}
if (targets.length > 1) {
  throw new Error("deploy --target accepts one platform");
}
if (values.host && targets.length > 0) {
  throw new Error("--host and --target cannot be combined");
}

let platforms: Platform[] | undefined;
if (values.host) {
  const host = `${process.platform}-${process.arch}`;
  if (!isPlatform(host)) {
    throw new Error(`no dotfmt target for ${host}`);
  }
  platforms = [host];
} else if (targets.length > 0) {
  platforms = targets;
}
const vsceTarget = platforms?.[0];

if (!values["skip-build"]) {
  await build(platforms);
}

const packaged = artifactPlatforms(platforms ?? []);
const missing = missingArtifacts(root, packaged, existsSync);
if (missing.length > 0) {
  const files = missing.map((file) => path.relative(root, file)).join(", ");
  throw new Error(
    `refusing to package: missing ${files}; run \`bun run build\` (or build the requested target) first`,
  );
}

if (values["skip-build"]) {
  const newest = await newestSourceMtime();
  const stale = staleArtifacts(root, packaged, newest, (file) =>
    statSync(file).mtimeMs,
  );
  if (stale.length > 0) {
    const files = stale.map((file) => path.relative(root, file)).join(", ");
    throw new Error(
      `refusing to package stale artifacts: ${files} are older than the newest source; run \`bun run build\` (without --skip-build) first`,
    );
  }
}

const vsix = path.join(
  root,
  vsceTarget ? `dotfmt-${vsceTarget}.vsix` : "dotfmt.vsix",
);
if (vsceTarget) {
  await $`bun x vsce package --no-dependencies --skip-license --target ${vsceTarget} --out ${vsix}`.cwd(
    root,
  );
} else {
  await $`bun x vsce package --no-dependencies --skip-license --out ${vsix}`.cwd(
    root,
  );
}

let remoteInstall: { host: string; cli: string; upload: string } | undefined;
if (values.remote) {
  const host = values.remote;
  const [remote, platform] = await Promise.all([
    remoteServer(host),
    remotePlatform(host),
  ]);
  if (!remote) {
    throw new Error(
      `no VS Code server found on ${host}; connect with Remote SSH first`,
    );
  }
  await checkRemotePlatform(host, platform, vsix, vsceTarget);
  remoteInstall = {
    host,
    cli: `~/.vscode-server/cli/servers/${remote.directory}/server/bin/code-server`,
    upload: `/tmp/dotfmt-${remote.commit}.vsix`,
  };
}

const local = localInstallDecision(vsceTarget, hostTarget(), packaged);
if (!local.install) {
  console.log(`dotfmt: ${local.reason}`);
} else {
  const installers = [Bun.which("code"), await localServer()].filter(
    (cli): cli is string => Boolean(cli),
  );
  if (installers.length === 0 && !values.remote) {
    throw new Error("VS Code not found");
  }
  for (const cli of installers) {
    await $`${cli} --install-extension ${vsix} --force`;
  }
}

if (remoteInstall) {
  const { host, cli, upload } = remoteInstall;
  await $`scp -q ${vsix} ${host}:${upload}`;
  await $`ssh ${host} ${`test -x ${cli} || { echo "no VS Code server at ${cli}; connect with Remote SSH first" >&2; exit 1; }; ${cli} --install-extension ${upload} --force; installed=$?; rm -f ${upload}; exit $installed`}`;
}

async function newestSourceMtime(): Promise<number> {
  const sources = [
    path.join(root, "src"),
    path.resolve(root, "../../../rust/crates/dotfmt"),
  ];
  let newest = 0;
  const visit = async (file: string): Promise<void> => {
    const info = await stat(file).catch(() => undefined);
    if (!info) {
      return;
    }
    if (info.isDirectory()) {
      for (const entry of await readdir(file)) {
        if (entry === "target" || entry === "node_modules" || entry === "dist") {
          continue;
        }
        await visit(path.join(file, entry));
      }
      return;
    }
    newest = Math.max(newest, info.mtimeMs);
  };
  for (const source of sources) {
    await visit(source);
  }
  for (const file of [
    "package.json",
    "schema.json",
    "language-configuration.json",
  ]) {
    const info = await stat(path.join(root, file)).catch(() => undefined);
    if (info) {
      newest = Math.max(newest, info.mtimeMs);
    }
  }
  return newest;
}

interface RemoteServer {
  commit: string;
  directory: string;
}

interface RemotePlatform {
  system: string;
  machine: string;
  target?: Platform;
}

async function checkRemotePlatform(
  host: string,
  platform: RemotePlatform | undefined,
  vsix: string,
  requested: Platform | undefined,
): Promise<void> {
  const name = path.basename(vsix);
  const label = platform
    ? `${platform.system} ${platform.machine}`
    : `\`uname -s -m\` on ${host}`;
  if (!platform) {
    throw new Error(
      `cannot determine the platform of ${host}; check that \`ssh ${host} uname -s -m\` works`,
    );
  }
  const shipped = await packagedPlatforms();
  if (shipped.length === 0) {
    throw new Error(
      `refusing to install ${name} on ${host}: the vsix has no bundled server binaries; run bun run build first`,
    );
  }
  if (!platform.target) {
    throw new Error(
      `refusing to install ${name} on ${host}: ${label} is not a dotfmt target (${Object.keys(
        TARGETS,
      ).join(", ")})`,
    );
  }
  if (requested && requested !== platform.target) {
    throw new Error(
      `refusing to install ${name} on ${host}: it was packaged with --target ${requested}, but ${label} is ${platform.target}`,
    );
  }
  if (!shipped.includes(platform.target)) {
    throw new Error(
      `refusing to install ${name} on ${host}: the vsix packages ${shipped.join(
        ", ",
      )}, not ${platform.target} for ${label}. Build with \`--target ${
        platform.target
      }\` or the default universal package.`,
    );
  }
}

async function remotePlatform(
  host: string,
): Promise<RemotePlatform | undefined> {
  const result = await $`ssh ${host} uname -s -m`.quiet().nothrow();
  if (result.exitCode !== 0) {
    return undefined;
  }
  const [system, machine] = result.stdout.toString().trim().split(/\s+/);
  if (!system || !machine) {
    return undefined;
  }
  return { system, machine, target: remoteTarget(system, machine) };
}

function remoteTarget(system: string, machine: string): Platform | undefined {
  const os = system.toLowerCase();
  const arch = machine.toLowerCase();
  if (arch === "x86_64" || arch === "amd64" || arch === "x64") {
    if (os === "linux" || os === "gnu/linux") {
      return "linux-x64";
    }
    if (
      os.startsWith("mingw") ||
      os.startsWith("msys") ||
      os.startsWith("cygwin")
    ) {
      return "win32-x64";
    }
  }
  if ((arch === "arm64" || arch === "aarch64") && os === "darwin") {
    return "darwin-arm64";
  }
  return undefined;
}

async function packagedPlatforms(): Promise<Platform[]> {
  let entries;
  try {
    entries = await readdir(path.join(root, "bin"), { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((entry) => entry.isDirectory() && isPlatform(entry.name))
    .map((entry) => entry.name as Platform)
    .sort();
}

async function remoteServer(host: string): Promise<RemoteServer | undefined> {
  const directories = await remoteDirectories(host);
  const version = await $`ssh ${host} code --version`.quiet().nothrow();
  const reported =
    version.exitCode === 0
      ? commitFromVersion(version.stdout.toString())
      : undefined;
  const directory =
    (reported
      ? directories.find((name) => name.endsWith(`-${reported}`))
      : undefined) ?? directories[0];
  if (!directory) {
    return undefined;
  }
  return {
    commit: directory.slice(directory.indexOf("-") + 1),
    directory,
  };
}

async function remoteDirectories(host: string): Promise<string[]> {
  const lru = await $`ssh ${host} cat ~/.vscode-server/cli/servers/lru.json`
    .quiet()
    .nothrow();
  if (lru.exitCode === 0) {
    try {
      const parsed = z
        .array(z.string())
        .safeParse(JSON.parse(lru.stdout.toString()));
      if (parsed.success) {
        const directories = parsed.data.filter(isServerDirectory);
        if (directories.length > 0) {
          return directories;
        }
      }
    } catch {
    }
  }
  const listing = await $`ssh ${host} ls -1t ~/.vscode-server/cli/servers`
    .quiet()
    .nothrow();
  if (listing.exitCode !== 0) {
    return [];
  }
  return listing.stdout
    .toString()
    .split(/\r?\n/)
    .map((name) => name.trim())
    .filter(isServerDirectory);
}

function isServerDirectory(name: string): boolean {
  return /^(?:Stable|Insiders)-[0-9a-f]{7,40}$/i.test(name);
}

function commitFromVersion(output: string): string | undefined {
  const commit = output.split(/\r?\n/)[1]?.trim();
  return commit && /^[0-9a-f]{7,40}$/i.test(commit) ? commit : undefined;
}

async function localServer(): Promise<string | undefined> {
  const servers = path.join(os.homedir(), ".vscode-server", "cli", "servers");
  const lru = Bun.file(path.join(servers, "lru.json"));
  if (await lru.exists()) {
    try {
      const parsed = z.array(z.string()).safeParse(await lru.json());
      if (parsed.success) {
        for (const directory of parsed.data.filter(isServerDirectory)) {
          const cli = serverCli(servers, directory);
          if (await Bun.file(cli).exists()) {
            return cli;
          }
        }
      }
    } catch {
    }
  }
  for (const directory of await recentServerDirectories(servers)) {
    const cli = serverCli(servers, directory);
    if (await Bun.file(cli).exists()) {
      return cli;
    }
  }
  return undefined;
}

function serverCli(servers: string, directory: string): string {
  return path.join(servers, directory, "server", "bin", "code-server");
}

async function recentServerDirectories(servers: string): Promise<string[]> {
  let entries;
  try {
    entries = await readdir(servers, { withFileTypes: true });
  } catch {
    return [];
  }
  const directories = await Promise.all(
    entries
      .filter((entry) => entry.isDirectory() && isServerDirectory(entry.name))
      .map(async (entry) => ({
        name: entry.name,
        modified: await stat(path.join(servers, entry.name))
          .then((stats) => stats.mtimeMs)
          .catch(() => 0),
      })),
  );
  return directories
    .sort((left, right) => right.modified - left.modified)
    .map((entry) => entry.name);
}
