import * as vscode from "vscode";
import {
  binaryVersion,
  Client,
  cliPath,
  extensionVersion,
  message,
} from "./client";
import { registerCommands } from "./commands";
import { SECTION, SELECTOR } from "./lib/languages";
import { tildify } from "./lib/paths";
import { PasteFormatter } from "./paste";
import { Status } from "./status";

let manager: Client | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const log = vscode.window.createOutputChannel("dotfmt", { log: true });
  const trace = vscode.window.createOutputChannel("dotfmt trace", {
    log: true,
  });
  const client = new Client(context, log, trace);
  const version = extensionVersion(context);
  const cli = cliPath(context);
  const status = new Status(client, context, version);
  manager = client;

  context.subscriptions.push(
    log,
    trace,
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
      if (
        event.affectsConfiguration(`${SECTION}.server.path`) ||
        event.affectsConfiguration(`${SECTION}.typescript.tsdk`)
      ) {
        void client.restart().then(
          () => status.invalidate(),
          (error: unknown) => {
            log.error(`server restart failed: ${message(error)}`);
            status.setError(`server failed to restart: ${message(error)}`);
            void vscode.window.showErrorMessage(
              `dotfmt language server failed to restart: ${message(error)}`,
            );
          },
        );
      } else if (event.affectsConfiguration(SECTION)) {
        status.invalidate();
      }
    }),
    vscode.workspace.onDidGrantWorkspaceTrust(() => {
      void client.restart().then(
        () => status.invalidate(),
        (error: unknown) => {
          log.error(`server restart failed: ${message(error)}`);
          status.setError(`server failed to restart: ${message(error)}`);
          void vscode.window.showErrorMessage(
            `dotfmt language server failed to restart: ${message(error)}`,
          );
        },
      );
    }),
  );
  registerCommands(context, client, log, status);

  log.info(`extension: ${context.extension.id} ${version}`);
  log.info(`CLI: ${tildify(cli)}`);
  void binaryVersion(cli).then((found) => {
    if (found) {
      log.info(`dotfmt version: ${found}`);
    }
  });
  client.start().catch((error: unknown) => {
    const failure = `${tildify(client.serverPath())}: ${message(error)}`;
    log.error(`language server failed to start: ${failure}`);
    status.setError(`server failed to start: ${message(error)}`);
    void vscode.window.showErrorMessage(
      `dotfmt language server failed to start (${failure}). Set "dotfmt.server.path" to a working binary or run "dotfmt: Doctor".`,
    );
  });
  void status.refresh();
}

export async function deactivate(): Promise<void> {
  await manager?.stop();
  manager = undefined;
}
