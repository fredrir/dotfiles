import * as vscode from "vscode";
import { registerCommands } from "./commands";
import { registerConfigWatcher } from "./config";
import { Server } from "./server";

let server: Server | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const outputChannel = vscode.window.createOutputChannel("Dot Code", {
    log: true,
  });
  context.subscriptions.push(outputChannel);
  server = new Server(context, outputChannel);

  registerCommands(context, server, outputChannel);
  registerConfigWatcher(context, server);
}

export async function deactivate(): Promise<void> {
  await server?.stop();
}
