import * as vscode from "vscode";
import { message, type Client } from "./client";
import { formatTextParams } from "./lib/paste";

export class PasteFormatter implements vscode.DocumentPasteEditProvider {
  public constructor(private readonly client: Client) {}

  public async provideDocumentPasteEdits(
    document: vscode.TextDocument,
    _ranges: readonly vscode.Range[],
    dataTransfer: vscode.DataTransfer,
    _context: vscode.DocumentPasteEditContext,
    token: vscode.CancellationToken,
  ): Promise<vscode.DocumentPasteEdit[] | undefined> {
    if (token.isCancellationRequested) {
      return undefined;
    }
    const item = dataTransfer.get("text/plain");
    if (!item) {
      return undefined;
    }
    const text = await item.asString();
    if (!text.trim()) {
      return undefined;
    }
    const params = formatTextParams(
      text,
      document.languageId,
      document.uri.toString(),
    );
    if (!params) {
      return undefined;
    }
    try {
      const formatted = await this.client.request<string>(
        "dotfmt/formatText",
        params,
      );
      if (!formatted || formatted === text) {
        return undefined;
      }
      return [
        new vscode.DocumentPasteEdit(
          formatted,
          "Format with dotfmt",
          vscode.DocumentDropOrPasteEditKind.Text,
        ),
      ];
    } catch (error) {
      console.warn(`dotfmt paste formatting skipped: ${message(error)}`);
      return undefined;
    }
  }
}
