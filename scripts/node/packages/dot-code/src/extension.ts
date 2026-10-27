import * as vscode from "vscode";
import { ClientManager } from "./client";
import { registerCommands } from "./commands";
import { registerConfigWatcher } from "./config";

let clientManager: ClientManager | undefined;

export async function activate(
  context: vscode.ExtensionContext,
): Promise<void> {
  const outputChannel = vscode.window.createOutputChannel("Dot Code", {
    log: true,
  });
  context.subscriptions.push(outputChannel);
  clientManager = new ClientManager(context, outputChannel);

  registerCommands(context, clientManager, outputChannel);
  registerConfigWatcher(context, clientManager);
  await clientManager.start();
}

export async function deactivate(): Promise<void> {
  await clientManager?.stop();
}
