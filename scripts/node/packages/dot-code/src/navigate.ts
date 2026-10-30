import * as vscode from "vscode";
import { SIBLING_FILE } from "./lib/constants";
import { activeFile } from "./lib/editor";
import type { Server } from "./server";

export async function navigate(server: Server, offset: 1 | -1): Promise<void> {
  const current = activeFile();
  if (current?.scheme !== "file") {
    return;
  }

  const dir = vscode.Uri.joinPath(current, "..");
  const explorer = vscode.workspace.getConfiguration("explorer", dir);
  const target = await server.request<string | null>(SIBLING_FILE, {
    path: current.fsPath,
    offset,
    root: vscode.workspace.getWorkspaceFolder(dir)?.uri.fsPath,
    locale: vscode.env.language,
    explorer: {
      sortOrder: explorer.get("sortOrder"),
      sortOrderLexicographicOptions: explorer.get(
        "sortOrderLexicographicOptions",
      ),
      sortOrderReverse: explorer.get("sortOrderReverse"),
    },
    exclude: vscode.workspace.getConfiguration("files", dir).get("exclude"),
  });
  if (target) {
    await vscode.commands.executeCommand(
      "vscode.open",
      vscode.Uri.file(target),
    );
  }
}
