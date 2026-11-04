import * as vscode from "vscode";
import type { Dotfmt } from "./dotfmt";
import { hunks, type Hunk } from "./hunks";

export class Formatter
  implements
    vscode.DocumentFormattingEditProvider,
    vscode.DocumentRangeFormattingEditProvider
{
  public constructor(private readonly dotfmt: Dotfmt) {}

  public async provideDocumentFormattingEdits(
    document: vscode.TextDocument,
    _options: vscode.FormattingOptions,
    token: vscode.CancellationToken,
  ): Promise<vscode.TextEdit[]> {
    const input = document.getText();
    const output = await this.dotfmt.format(document, input, token);
    if (output === undefined || output === input) {
      return [];
    }
    const edits = [
      vscode.TextEdit.replace(
        document.validateRange(new vscode.Range(0, 0, document.lineCount, 0)),
        output,
      ),
    ];
    const eol = endOfLine(output);
    if (eol !== undefined && eol !== document.eol) {
      edits.push(vscode.TextEdit.setEndOfLine(eol));
    }
    return edits;
  }

  public provideDocumentRangeFormattingEdits(
    document: vscode.TextDocument,
    range: vscode.Range,
    options: vscode.FormattingOptions,
    token: vscode.CancellationToken,
  ): Promise<vscode.TextEdit[]> {
    return this.provideDocumentRangesFormattingEdits(
      document,
      [range],
      options,
      token,
    );
  }

  /** dotfmt formats whole files, so only the changed hunks touching a range are applied. */
  public async provideDocumentRangesFormattingEdits(
    document: vscode.TextDocument,
    ranges: vscode.Range[],
    _options: vscode.FormattingOptions,
    token: vscode.CancellationToken,
  ): Promise<vscode.TextEdit[]> {
    const input = document.getText();
    const output = await this.dotfmt.format(document, input, token);
    if (output === undefined || output === input) {
      return [];
    }
    const eol = document.eol === vscode.EndOfLine.CRLF ? "\r\n" : "\n";
    return hunks(input, output.replace(/\r?\n/g, eol))
      .filter((hunk) => ranges.some((range) => touches(hunk, range)))
      .map((hunk) =>
        vscode.TextEdit.replace(
          document.validateRange(new vscode.Range(hunk.start, 0, hunk.end, 0)),
          hunk.text,
        ),
      );
  }
}

function touches(hunk: Hunk, range: vscode.Range): boolean {
  const last =
    range.end.line > range.start.line && range.end.character === 0
      ? range.end.line - 1
      : range.end.line;
  return (
    hunk.start <= last && Math.max(hunk.end, hunk.start + 1) > range.start.line
  );
}

function endOfLine(text: string): vscode.EndOfLine | undefined {
  const crlf = text.includes("\r\n");
  const lf = /(?:^|[^\r])\n/.test(text);
  if (crlf === lf) {
    return undefined;
  }
  return crlf ? vscode.EndOfLine.CRLF : vscode.EndOfLine.LF;
}
