import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";
import { $ } from "bun";
import { z } from "zod";
import { build, isPlatform, TARGETS, type Platform } from "./build.ts";

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

const installers = [Bun.which("code"), await localServer()].filter(
  (cli): cli is string => Boolean(cli),
);
if (installers.length === 0 && !values.remote) {
  throw new Error("VS Code not found");
}
for (const cli of installers) {
  await $`${cli} --install-extension ${vsix} --force`;
}

if (values.remote) {
  const host = values.remote;
  const remote = await remoteServer(host);
  if (!remote) {
    throw new Error(
      `no VS Code server found on ${host}; connect with Remote SSH first`,
    );
  }
  const cli = `~/.vscode-server/cli/servers/${remote.directory}/server/bin/code-server`;
  const upload = `/tmp/dotfmt-${remote.commit}.vsix`;
  await $`scp -q ${vsix} ${host}:${upload}`;
  await $`ssh ${host} ${`test -x ${cli} || { echo "no VS Code server at ${cli}; connect with Remote SSH first" >&2; exit 1; }; ${cli} --install-extension ${upload} --force; installed=$?; rm -f ${upload}; exit $installed`}`;
}

interface RemoteServer {
  commit: string;
  directory: string;
}

/** The remote VS Code server's own commit, not the local desktop commit. */
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
      // Fall through to listing the server directory.
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
  if (!(await lru.exists())) {
    return undefined;
  }
  const [latest] = z.array(z.string()).parse(await lru.json());
  if (!latest) {
    return undefined;
  }
  const cli = path.join(servers, latest, "server", "bin", "code-server");
  return (await Bun.file(cli).exists()) ? cli : undefined;
}
