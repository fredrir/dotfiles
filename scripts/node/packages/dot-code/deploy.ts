import path from "node:path";
import { parseArgs } from "node:util";
import { $ } from "bun";
import { build } from "./build.ts";

const { values } = parseArgs({ options: { remote: { type: "string" } } });

const root = import.meta.dir;
const vsix = path.join(root, "dot-code.vsix");

await build();
await $`bun x vsce package --no-dependencies --skip-license --out ${vsix}`.cwd(
  root,
);
await $`code --install-extension ${vsix} --force`;

if (values.remote) {
  const host = values.remote;
  const commit = (await $`code --version`.text()).split("\n")[1]?.trim();
  if (!commit) {
    throw new Error("VS Code commit not found");
  }

  const cli = `~/.vscode-server/cli/servers/Stable-${commit}/server/bin/code-server`;
  const upload = "/tmp/dot-code.vsix";
  await $`scp -q ${vsix} ${host}:${upload}`;
  await $`ssh ${host} ${`test -x ${cli} || { echo "no VS Code server for ${commit}; connect with Remote SSH first" >&2; exit 1; }; ${cli} --install-extension ${upload} --force; installed=$?; rm -f ${upload}; exit $installed`}`;
}
