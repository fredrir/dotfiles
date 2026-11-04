import * as vscode from "vscode";
import { FILE_MARKDOWN } from "./lib/constants";
import type { Server } from "./server";

export async function copyFileAsMarkdown(server: Server): Promise<void> {
  const document = vscode.window.activeTextEditor?.document;
  if (!document) {
    return;
  }
  const markdown = await server.request<string>(FILE_MARKDOWN, {
    path: document.uri.fsPath,
    root: vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath,
    text: document.getText(),
  });
  await vscode.env.clipboard.writeText(markdown);
}
