import * as vscode from "vscode";
import { SECTION } from "./lib/constants";
import type { Server } from "./server";

export function registerConfigWatcher(
  context: vscode.ExtensionContext,
  server: Server,
): void {
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration(`${SECTION}.server.path`)) {
        await server.restart();
      }
    }),
  );
}
