import * as cp from "node:child_process";
import * as util from "node:util";
import * as vscode from "vscode";
import { copyFileAsMarkdown } from "./copy";
import { install } from "./install";
import { navigate } from "./navigate";
import { purgeComments } from "./purge";
import { rename } from "./rename";
import type { Server } from "./server";
import { transformCase, type Case, type Target } from "./transform";

const execFile = util.promisify(cp.execFile);

const CODE_ACTIONS = {
  "dotCode.extractInterface": "refactor.extract.interface.dotCode",
  "dotCode.extractType": "refactor.extract.type.dotCode",
  "dotCode.convertType": "refactor.rewrite.typeDefinition",
};

const CASES = {
  Camelcase: "camel",
  Kebabcase: "kebab",
  Lowercase: "lower",
  Pascalcase: "pascal",
  Snakecase: "snake",
  Titlecase: "title",
  Uppercase: "upper",
} as const satisfies Record<string, Case>;

export function registerCommands(
  context: vscode.ExtensionContext,
  server: Server,
  outputChannel: vscode.LogOutputChannel,
): void {
  context.subscriptions.push(
    ...Object.entries(CODE_ACTIONS).map(([command, kind]) =>
      vscode.commands.registerCommand(command, () =>
        vscode.commands.executeCommand("editor.action.codeAction", {
          kind,
          apply: "first",
        }),
      ),
    ),
    ...Object.entries(CASES).map(([name, kind]) =>
      vscode.commands.registerCommand(
        `dotCode.transformTo${name}`,
        (args?: { target?: Target }) =>
          transformCase(server, kind, args?.target ?? "editor"),
      ),
    ),
    vscode.commands.registerCommand("dotCode.rename", rename),
    vscode.commands.registerCommand("dotCode.nextFile", () =>
      navigate(server, 1),
    ),
    vscode.commands.registerCommand("dotCode.previousFile", () =>
      navigate(server, -1),
    ),
    vscode.commands.registerCommand("dotCode.copyFileAsMarkdown", () =>
      copyFileAsMarkdown(server),
    ),
    vscode.commands.registerCommand("dotCode.purgeComments", () =>
      purgeComments(server),
    ),
    vscode.commands.registerCommand("dotCode.install", () =>
      install(server, outputChannel),
    ),
    vscode.commands.registerCommand("dotCode.restart", () => server.restart()),
    vscode.commands.registerCommand("dotCode.showVersion", async () => {
      const serverPath = await server.serverPath();
      try {
        const { stdout } = await execFile(serverPath, ["--version"]);
        const version = stdout.trim();
        outputChannel.info(`${version} (${serverPath})`);
        void vscode.window.showInformationMessage(version);
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        outputChannel.error(`version check failed: ${msg}`);
        void vscode.window.showErrorMessage(`version check failed: ${msg}`);
      }
    }),
  );
}
