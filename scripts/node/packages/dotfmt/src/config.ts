import os from "node:os";
import path from "node:path";
import * as vscode from "vscode";
import type { Dotfmt } from "./dotfmt";
import { CONFIG_NAME, SECTION } from "./lib/languages";
import type { Reporter } from "./report";
import type { Status } from "./status";

const SETTLE_MS = 100;

/** Revalidates dotfmt.dotfile files as they change, so Problems and status follow them. */
export function watchConfigs(
  dotfmt: Dotfmt,
  reporter: Reporter,
  status: Status,
): vscode.Disposable[] {
  const pending = new Map<string, NodeJS.Timeout>();
  const schedule = (uri: vscode.Uri, action: () => Promise<void>): void => {
    const key = uri.toString();
    clearTimeout(pending.get(key));
    pending.set(
      key,
      setTimeout(() => {
        pending.delete(key);
        void action().then(() => status.invalidate());
      }, SETTLE_MS),
    );
  };
  const changed = (uri: vscode.Uri): void =>
    schedule(uri, () => dotfmt.validate(uri));
  const deleted = (uri: vscode.Uri): void =>
    schedule(uri, () => {
      reporter.forget(uri);
      return Promise.resolve();
    });

  const global = path.join(
    process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"),
    "dotfmt",
  );
  const watchers = [
    vscode.workspace.createFileSystemWatcher(`**/${CONFIG_NAME}`),
    vscode.workspace.createFileSystemWatcher(
      new vscode.RelativePattern(vscode.Uri.file(global), CONFIG_NAME),
    ),
  ];
  for (const document of vscode.workspace.textDocuments) {
    if (isConfig(document.uri)) {
      changed(document.uri);
    }
  }

  return [
    ...watchers,
    ...watchers.flatMap((watcher) => [
      watcher.onDidCreate(changed),
      watcher.onDidChange(changed),
      watcher.onDidDelete(deleted),
    ]),
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (isConfig(document.uri)) {
        changed(document.uri);
      }
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration(`${SECTION}.path`)) {
        status.invalidate();
      }
    }),
    {
      dispose: () => {
        for (const timer of pending.values()) {
          clearTimeout(timer);
        }
      },
    },
  ];
}

function isConfig(uri: vscode.Uri): boolean {
  return uri.scheme === "file" && path.basename(uri.fsPath) === CONFIG_NAME;
}
