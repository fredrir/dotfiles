import fs from "node:fs/promises";
import path from "node:path";
import type { Agent } from "package-manager-detector";
import { resolveCommand } from "package-manager-detector/commands";
import { detect } from "package-manager-detector/detect";
import * as vscode from "vscode";
import { SECTION } from "./lib/constants";
import { activeFile } from "./lib/editor";

const AGENTS: Agent[] = ["npm", "pnpm", "yarn", "bun", "deno"];

interface Project {
  cwd: string;
  agent?: Agent;
}

let installing = false;

const exists = (file: string) =>
  fs.access(file).then(
    () => true,
    () => false,
  );

function startDir(): string | undefined {
  const file = activeFile();
  return file?.scheme === "file"
    ? path.dirname(file.fsPath)
    : vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

async function findProject(start: string): Promise<Project | undefined> {
  let nearest: string | undefined;
  for (let dir = start; ; dir = path.dirname(dir)) {
    const detected = await detect({ cwd: dir, stopDir: dir });
    if (detected) {
      return { cwd: dir, agent: detected.agent };
    }
    if (!nearest && (await exists(path.join(dir, "package.json")))) {
      nearest = dir;
    }
    if (path.dirname(dir) === dir) {
      return nearest ? { cwd: nearest } : undefined;
    }
  }
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
  outputChannel: vscode.LogOutputChannel,
): Promise<void> {
  if (installing) {
    return;
  }
  installing = true;
  try {
    const start = startDir();
    const project = start === undefined ? undefined : await findProject(start);
    if (!project) {
      void vscode.window.showErrorMessage("package.json not found");
      return;
    }

    const agent =
      project.agent ??
      ((await vscode.window.showQuickPick(AGENTS, {
        placeHolder: "package manager",
      })) as Agent | undefined);
    const resolved = agent && resolveCommand(agent, "install", []);
    if (!resolved) {
      return;
    }

    if (await run(resolved.command, resolved.args, project.cwd)) {
      await refresh(outputChannel);
    }
  } finally {
    installing = false;
  }
}
