import * as vscode from "vscode";
import { SECTION } from "./lib/constants";
import type { ClientManager } from "./client";

export function registerConfigWatcher(
  context: vscode.ExtensionContext,
  clientManager: ClientManager,
): void {
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration(`${SECTION}.server.path`)) {
        await clientManager.restart();
      }
    }),
  );
}
