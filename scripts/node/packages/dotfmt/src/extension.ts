import * as vscode from "vscode";
import { Client, cliPath, message } from "./client";
import { registerCommands } from "./commands";
import { PasteFormatter } from "./paste";
import { SECTION, SELECTOR } from "./lib/languages";
import { Status } from "./status";

let manager: Client | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const log = vscode.window.createOutputChannel("dotfmt", { log: true });
  const client = new Client(context, log);
  const status = new Status(client, cliPath(context));
  manager = client;

  context.subscriptions.push(
    log,
    client,
    status,
    vscode.languages.registerDocumentPasteEditProvider(
      SELECTOR,
      new PasteFormatter(client),
      {
        providedPasteEditKinds: [vscode.DocumentDropOrPasteEditKind.Text],
      },
    ),
    vscode.window.onDidChangeActiveTextEditor(() => void status.refresh()),
    vscode.workspace.onDidOpenTextDocument((document) => {
      if (document === vscode.window.activeTextEditor?.document) {
        void status.refresh();
      }
    }),
    vscode.workspace.onDidSaveTextDocument(() => {
      status.invalidate();
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration(`${SECTION}.server.path`)) {
        void client.restart().then(
          () => status.invalidate(),
          (error: unknown) =>
            log.error(`server restart failed: ${message(error)}`),
        );
      } else if (event.affectsConfiguration(SECTION)) {
        status.invalidate();
      }
    }),
    vscode.workspace.onDidGrantWorkspaceTrust(() => {
      void client.restart().then(
        () => status.invalidate(),
        (error: unknown) =>
          log.error(`server restart failed: ${message(error)}`),
      );
    }),
  );
  registerCommands(context, client, log);

  log.info(`CLI: ${cliPath(context)}`);
  client.start().catch((error: unknown) => {
    log.error(`language server failed to start: ${message(error)}`);
  });
  void status.refresh();
}

export async function deactivate(): Promise<void> {
  await manager?.stop();
  manager = undefined;
}
