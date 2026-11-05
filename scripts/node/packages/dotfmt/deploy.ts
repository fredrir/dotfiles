import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";
import { $ } from "bun";
import { z } from "zod";
import { build } from "./build.ts";

const { values } = parseArgs({ options: { remote: { type: "string" } } });

const root = import.meta.dir;
const vsix = path.join(root, "dotfmt.vsix");

await build();
await $`bun x vsce package --no-dependencies --skip-license --out ${vsix}`.cwd(
  root,
);

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
  const commit = (await $`code --version`.text()).split("\n")[1]?.trim();
  if (!commit) {
    throw new Error("VS Code commit not found");
  }

  const cli = `~/.vscode-server/cli/servers/Stable-${commit}/server/bin/code-server`;
  const upload = "/tmp/dotfmt.vsix";
  await $`scp -q ${vsix} ${host}:${upload}`;
  await $`ssh ${host} ${`test -x ${cli} || { echo "no VS Code server for ${commit}; connect with Remote SSH first" >&2; exit 1; }; ${cli} --install-extension ${upload} --force; installed=$?; rm -f ${upload}; exit $installed`}`;
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
