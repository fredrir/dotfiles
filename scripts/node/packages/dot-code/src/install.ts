import path from "node:path";
import * as vscode from "vscode";
import { PACKAGE_MANAGER, SECTION } from "./lib/constants";
import { activeFile } from "./lib/editor";
import type { Server } from "./server";

interface Plan {
  cwd: string;
  command?: [string, ...string[]];
  choices?: string[];
}

let installing = false;

function startDir(): string | undefined {
  const file = activeFile();
  return file?.scheme === "file"
    ? path.dirname(file.fsPath)
    : vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

async function choose(
  server: Server,
  dir: string,
  choices: string[],
): Promise<Plan | null> {
  const agent = await vscode.window.showQuickPick(choices, {
    placeHolder: "package manager",
  });
  return agent
    ? server.request<Plan | null>(PACKAGE_MANAGER, { path: dir, agent })
    : null;
}

async function run(
  command: string,
  args: string[],
  cwd: string,
): Promise<boolean> {
  const name = [command, ...args].join(" ");
  const task = new vscode.Task(
    { type: SECTION },
    vscode.workspace.getWorkspaceFolder(vscode.Uri.file(cwd)) ??
      vscode.TaskScope.Workspace,
    name,
    "Dot Code",
    new vscode.ShellExecution(command, args, { cwd }),
  );
  task.presentationOptions = {
    reveal: vscode.TaskRevealKind.Silent,
    panel: vscode.TaskPanelKind.New,
    close: false,
  };

  const terminals: vscode.Terminal[] = [];
  const subscriptions = [
    vscode.window.onDidOpenTerminal((terminal) => {
      if (terminal.name.includes(name)) {
        terminals.push(terminal);
      }
    }),
  ];
  try {
    const execution = await vscode.tasks.executeTask(task);
    const exitCode = await new Promise<number | undefined>((resolve) => {
      subscriptions.push(
        vscode.tasks.onDidEndTaskProcess((event) => {
          if (event.execution === execution) {
            resolve(event.exitCode);
          }
        }),
        vscode.tasks.onDidEndTask((event) => {
          if (event.execution === execution) {
            resolve(undefined);
          }
        }),
      );
    });
    if (exitCode !== 0) {
      return false;
    }
    for (const terminal of terminals) {
      terminal.dispose();
    }
    return true;
  } finally {
    for (const subscription of subscriptions) {
      subscription.dispose();
    }
  }
}

async function refresh(outputChannel: vscode.LogOutputChannel): Promise<void> {
  const commands = vscode.workspace
    .getConfiguration(SECTION)
    .get<string[]>("install.restart", []);
  const registered = new Set(await vscode.commands.getCommands(true));
  await Promise.all(
    commands
      .filter((command) => registered.has(command))
      .map((command) =>
        vscode.commands.executeCommand(command).then(undefined, (error) => {
          const msg = error instanceof Error ? error.message : String(error);
          outputChannel.warn(`${command} failed: ${msg}`);
        }),
      ),
  );
}

export async function install(
  server: Server,
  outputChannel: vscode.LogOutputChannel,
): Promise<void> {
  if (installing) {
    return;
  }
  installing = true;
  try {
    const dir = startDir();
    const detected =
      dir === undefined
        ? null
        : await server.request<Plan | null>(PACKAGE_MANAGER, { path: dir });
    if (!detected || dir === undefined) {
      void vscode.window.showErrorMessage("package.json not found");
      return;
    }

    const chosen = detected.command
      ? detected
      : await choose(server, dir, detected.choices ?? []);
    if (!chosen?.command) {
      return;
    }
    const [command, ...args] = chosen.command;
    if (await run(command, args, chosen.cwd)) {
      await refresh(outputChannel);
    }
  } finally {
    installing = false;
  }
}
