import { existsSync } from "node:fs";
import path from "node:path";
import * as vscode from "vscode";
import { message, type Client } from "./client";
import { CONFIG_NAME, LANGUAGES, SELECTOR } from "./lib/languages";

const SERVER_COMMANDS = {
  "dotfmt.fixAll": "dotfmt.fixAll",
  "dotfmt.organizeImports": "dotfmt.organizeImports",
  "dotfmt.sortTailwind": "dotfmt.sortTailwind",
} as const;

export function registerCommands(
  context: vscode.ExtensionContext,
  client: Client,
  log: vscode.LogOutputChannel,
): void {
  context.subscriptions.push(
    ...Object.keys(SERVER_COMMANDS).map((command) =>
      vscode.commands.registerCommand(command, () =>
        execute(client, command, log),
      ),
    ),
    vscode.commands.registerCommand("dotfmt.restartServer", async () => {
      log.info("restarting language server");
      try {
        await client.restart();
      } catch (error) {
        log.error(`server restart failed: ${message(error)}`);
      }
    }),
    vscode.commands.registerCommand("dotfmt.showOutput", () => log.show(true)),
    vscode.commands.registerCommand("dotfmt.setAsDefaultFormatter", () =>
      setAsDefaultFormatter(context),
    ),
    vscode.commands.registerCommand("dotfmt.openConfig", () => openConfig()),
  );
}

async function execute(
  client: Client,
  command: string,
  log: vscode.LogOutputChannel,
): Promise<void> {
  const document = vscode.window.activeTextEditor?.document;
  if (!document || vscode.languages.match(SELECTOR, document) === 0) {
    return;
  }
  try {
    await client.request("workspace/executeCommand", {
      command,
      arguments: [document.uri.toString()],
    });
  } catch (error) {
    log.error(`${command} failed: ${message(error)}`);
    void vscode.window.showErrorMessage(`dotfmt: ${message(error)}`);
  }
}

async function setAsDefaultFormatter(
  context: vscode.ExtensionContext,
): Promise<void> {
  await Promise.all(
    Object.keys(LANGUAGES).map((language) =>
      vscode.workspace
        .getConfiguration("editor", { languageId: language })
        .update(
          "defaultFormatter",
          context.extension.id,
          vscode.ConfigurationTarget.Workspace,
        ),
    ),
  );
  void vscode.window.showInformationMessage(
    "dotfmt is now the default formatter for its languages in this workspace.",
  );
}

async function openConfig(): Promise<void> {
  const document = vscode.window.activeTextEditor?.document;
  const folder = document
    ? vscode.workspace.getWorkspaceFolder(document.uri)
    : vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    void vscode.window.showWarningMessage(
      "dotfmt needs a workspace folder to find a dotfmt.dotfile.",
    );
    return;
  }
  const nearest = document
    ? findConfig(document.uri.fsPath, folder.uri.fsPath)
    : undefined;
  const target = nearest ?? path.join(folder.uri.fsPath, CONFIG_NAME);
  if (!existsSync(target)) {
    const create = "Create";
    const answer = await vscode.window.showInformationMessage(
      `No ${CONFIG_NAME} found. Create one in ${folder.name}?`,
      create,
    );
    if (answer !== create) {
      return;
    }
    await vscode.workspace.fs.writeFile(
      vscode.Uri.file(target),
      new TextEncoder().encode(""),
    );
  }
  await vscode.window.showTextDocument(vscode.Uri.file(target));
}

function findConfig(file: string, root: string): string | undefined {
  let directory = path.dirname(file);
  while (true) {
    const candidate = path.join(directory, CONFIG_NAME);
    if (existsSync(candidate)) {
      return candidate;
    }
    if (directory === root || !directory.startsWith(root)) {
      break;
    }
    const parent = path.dirname(directory);
    if (parent === directory) {
      break;
    }
    directory = parent;
  }
  return undefined;
}
