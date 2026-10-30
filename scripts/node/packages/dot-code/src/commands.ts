import * as cp from "node:child_process";
import * as util from "node:util";
import * as vscode from "vscode";
import type { ClientManager } from "./client";
import { install } from "./install";
import { navigate } from "./navigate";
import { rename } from "./rename";

const execFile = util.promisify(cp.execFile);

const CODE_ACTIONS = {
  "dotCode.extractInterface": "refactor.extract.interface.dotCode",
  "dotCode.extractType": "refactor.extract.type.dotCode",
  "dotCode.convertType": "refactor.rewrite.typeDefinition",
};

export function registerCommands(
  context: vscode.ExtensionContext,
  clientManager: ClientManager,
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
    vscode.commands.registerCommand("dotCode.rename", rename),
    vscode.commands.registerCommand("dotCode.nextFile", () => navigate(1)),
    vscode.commands.registerCommand("dotCode.previousFile", () => navigate(-1)),
    vscode.commands.registerCommand("dotCode.install", () =>
      install(outputChannel),
    ),
    vscode.commands.registerCommand("dotCode.restart", () =>
      clientManager.restart(),
    ),
    vscode.commands.registerCommand("dotCode.showVersion", async () => {
      const server = clientManager.serverPath();
      try {
        const { stdout } = await execFile(server, ["--version"]);
        const version = stdout.trim();
        outputChannel.info(`${version} (${server})`);
        void vscode.window.showInformationMessage(version);
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        outputChannel.error(`version check failed: ${msg}`);
        void vscode.window.showErrorMessage(`version check failed: ${msg}`);
      }
    }),
  );
}
