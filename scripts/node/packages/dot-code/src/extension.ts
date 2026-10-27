import path from "node:path";
import {
  type ExtensionContext,
  Position,
  Selection,
  commands,
  window,
  workspace,
} from "vscode";
import {
  LanguageClient,
  type LanguageClientOptions,
  TransportKind,
} from "vscode-languageclient/node";

const SECTION = "dotCode";
const LANGUAGES = ["typescript", "typescriptreact"];

let client: LanguageClient | undefined;

export async function activate(context: ExtensionContext): Promise<void> {
  context.subscriptions.push(
    commands.registerCommand("dotCode.rename", rename),
    commands.registerCommand("dotCode.restart", () => restart(context)),
    workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration(`${SECTION}.server.path`)) {
        await restart(context);
      }
    }),
  );
  await start(context);
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}

async function start(context: ExtensionContext): Promise<void> {
  const configured = workspace
    .getConfiguration(SECTION)
    .get<string>("server.path");
  const command =
    configured ||
    context.asAbsolutePath(
      path.join(
        "bin",
        `${process.platform}-${process.arch}`,
        "dot-code-server",
      ),
    );
  const options: LanguageClientOptions = {
    documentSelector: LANGUAGES.flatMap((language) => [
      { scheme: "file", language },
      { scheme: "untitled", language },
    ]),
    synchronize: { configurationSection: SECTION },
  };
  client = new LanguageClient(
    SECTION,
    "Dot Code",
    { command, transport: TransportKind.stdio },
    options,
  );
  await client.start();
}

async function restart(context: ExtensionContext): Promise<void> {
  await client?.stop();
  await start(context);
}

async function rename(
  uri: string,
  position: { line: number; character: number },
): Promise<void> {
  const editor = window.activeTextEditor;
  if (editor?.document.uri.toString() !== uri) {
    return;
  }
  const target = new Position(position.line, position.character);
  editor.selection = new Selection(target, target);
  await commands.executeCommand("editor.action.rename");
}
