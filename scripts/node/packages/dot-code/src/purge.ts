import * as vscode from "vscode";
import { PURGE_COMMENTS } from "./lib/constants";
import type { Server } from "./server";

interface Position {
  line: number;
  character: number;
}

interface Purged {
  edits: { range: { start: Position; end: Position }; newText: string }[];
  skipped?: string;
}

export async function purgeComments(server: Server): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return;
  }
  const { document } = editor;
  const version = document.version;
  const purged = await server.request<Purged>(PURGE_COMMENTS, {
    path: document.uri.fsPath,
    text: document.getText(),
  });
  if (purged.skipped) {
    void vscode.window.showWarningMessage(`doc-purge: ${purged.skipped}`);
    return;
  }
  if (purged.edits.length === 0 || document.version !== version) {
    return;
  }
  await editor.edit((builder) => {
    for (const { range, newText } of purged.edits) {
      builder.replace(
        new vscode.Range(
          range.start.line,
          range.start.character,
          range.end.line,
          range.end.character,
        ),
        newText,
      );
    }
  });
}
