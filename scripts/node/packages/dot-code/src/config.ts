import * as vscode from "vscode";
import type { ClientManager } from "./client";
import { SECTION } from "./lib/constants";

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
