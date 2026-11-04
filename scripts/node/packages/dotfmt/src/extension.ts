import * as vscode from "vscode";
import { watchConfigs } from "./config";
import { Dotfmt } from "./dotfmt";
import { Formatter } from "./format";
import { SELECTOR } from "./lib/languages";
import { Reporter } from "./report";
import { Status } from "./status";

export function activate(context: vscode.ExtensionContext): void {
  const log = vscode.window.createOutputChannel("dotfmt", { log: true });
  const reporter = new Reporter(log);
  const dotfmt = new Dotfmt(context.extensionPath, log, reporter);
  const status = new Status(dotfmt);
  const formatter = new Formatter(dotfmt);

  context.subscriptions.push(
    log,
    reporter,
    dotfmt,
    status,
    vscode.languages.registerDocumentFormattingEditProvider(
      SELECTOR,
      formatter,
    ),
    vscode.languages.registerDocumentRangeFormattingEditProvider(
      SELECTOR,
      formatter,
    ),
    vscode.commands.registerCommand("dotfmt.showOutput", () => log.show(true)),
    dotfmt.onDidFormat(({ document, failure }) =>
      status.formatted(document, failure),
    ),
    vscode.window.onDidChangeActiveTextEditor(() => void status.refresh()),
    vscode.workspace.onDidOpenTextDocument((document) => {
      if (document === vscode.window.activeTextEditor?.document) {
        void status.refresh();
      }
    }),
    ...watchConfigs(dotfmt, reporter, status),
  );
  void status.refresh();
}
