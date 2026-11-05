import { existsSync } from "node:fs";
import path from "node:path";
import * as vscode from "vscode";
import {
  binaryVersion,
  cliPath,
  extensionVersion,
  globalConfigFile,
  message,
  type Client,
} from "./client";
import { explain } from "./lib/explain";
import { CONFIG_NAME, LANGUAGES, SELECTOR } from "./lib/languages";
import { expandHome, findConfig, isWithin, tildify } from "./lib/paths";
import { readSettings } from "./lib/settings";

export function registerCommands(
  context: vscode.ExtensionContext,
  client: Client,
  log: vscode.LogOutputChannel,
): void {
  context.subscriptions.push(
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
    vscode.commands.registerCommand("dotfmt.showStatus", () =>
      showStatus(client),
    ),
    vscode.commands.registerCommand("dotfmt.doctor", () =>
      doctor(context, client, log),
    ),
  );
}

async function setAsDefaultFormatter(
  context: vscode.ExtensionContext,
): Promise<void> {
  const hasFolder = (vscode.workspace.workspaceFolders?.length ?? 0) > 0;
  const target = hasFolder
    ? vscode.ConfigurationTarget.Workspace
    : vscode.ConfigurationTarget.Global;
  const failures: string[] = [];
  await Promise.all(
    Object.keys(LANGUAGES).map(async (language) => {
      try {
        await vscode.workspace
          .getConfiguration("editor", { languageId: language })
          .update("defaultFormatter", context.extension.id, target);
      } catch (error) {
        failures.push(`${language}: ${message(error)}`);
      }
    }),
  );
  if (failures.length > 0) {
    void vscode.window.showWarningMessage(
      `dotfmt could not set the default formatter for ${failures.length} ${
        failures.length === 1 ? "language" : "languages"
      }. ${failures[0]}`,
    );
    return;
  }
  void vscode.window.showInformationMessage(
    `dotfmt is now the default formatter for its languages in ${
      hasFolder ? "this workspace" : "user settings"
    }.`,
  );
}

async function openConfig(): Promise<void> {
  const document = vscode.window.activeTextEditor?.document;
  const folder =
    (document
      ? vscode.workspace.getWorkspaceFolder(document.uri)
      : undefined) ?? vscode.workspace.workspaceFolders?.[0];
  const target = folder
    ? localConfig(document, folder.uri.fsPath)
    : globalConfigFile();
  if (!existsSync(target)) {
    const create = "Create";
    const answer = await vscode.window.showInformationMessage(
      `No ${CONFIG_NAME} found ${
        folder ? `in ${folder.name}` : "in the user config directory"
      }. Create one?`,
      create,
    );
    if (answer !== create) {
      return;
    }
    await vscode.workspace.fs.createDirectory(
      vscode.Uri.file(path.dirname(target)),
    );
    await vscode.workspace.fs.writeFile(
      vscode.Uri.file(target),
      new Uint8Array(),
    );
  }
  await vscode.window.showTextDocument(vscode.Uri.file(target));
}

function localConfig(
  document: vscode.TextDocument | undefined,
  root: string,
): string {
  const file =
    document &&
    document.uri.scheme !== "untitled" &&
    isWithin(root, document.uri.fsPath)
      ? document.uri.fsPath
      : undefined;
  return (
    (file ? findConfig(file, root, existsSync) : undefined) ??
    path.join(root, CONFIG_NAME)
  );
}

async function showStatus(client: Client): Promise<void> {
  const document = supportedDocument();
  if (!document) {
    void vscode.window.showInformationMessage(
      "dotfmt: no supported file is active.",
    );
    return;
  }
  try {
    const result = await explain(client, document.uri, document.languageId);
    if (result.error) {
      const showOutput = "Show Output";
      const answer = await vscode.window.showWarningMessage(
        `dotfmt: ${result.error}`,
        showOutput,
      );
      if (answer === showOutput) {
        await vscode.commands.executeCommand("dotfmt.showOutput");
      }
      return;
    }
    const detail = [
      result.language ?? "unmapped",
      result.dialect ? `(${result.dialect})` : undefined,
      result.skipped ? `skipped: ${result.skipped}` : undefined,
    ]
      .filter(Boolean)
      .join(" ");
    const nearest = result.configs.at(-1);
    const doctor = "Doctor";
    const answer = await vscode.window.showInformationMessage(
      `dotfmt: ${detail}${nearest ? ` · ${tildify(nearest)}` : ""}`,
      doctor,
    );
    if (answer === doctor) {
      await vscode.commands.executeCommand("dotfmt.doctor");
    }
  } catch (error) {
    void vscode.window.showWarningMessage(
      `dotfmt status unavailable: ${message(error)}`,
    );
  }
}

async function doctor(
  context: vscode.ExtensionContext,
  client: Client,
  log: vscode.LogOutputChannel,
): Promise<void> {
  const summary = await doctorSummary(context, client);
  log.info(summary);
  log.show(true);
  const copy = "Copy Summary";
  const answer = await vscode.window.showInformationMessage(
    "dotfmt doctor: full report in the dotfmt output channel.",
    copy,
  );
  if (answer === copy) {
    await vscode.env.clipboard.writeText(summary);
  }
}

async function doctorSummary(
  context: vscode.ExtensionContext,
  client: Client,
): Promise<string> {
  const version = extensionVersion(context);
  const cli = cliPath(context);
  const cliVersion = await binaryVersion(cli);
  const folders = vscode.workspace.workspaceFolders ?? [];
  const target = doctorTarget();
  const settings = readSettings(target.uri);
  const serverVersion = client.version();
  const lines = [
    "dotfmt doctor",
    `extension: ${context.extension.id} ${version}`,
    `server: ${tildify(client.serverPath())}${
      serverVersion
        ? ` (dotfmt-lsp ${serverVersion})`
        : " (version unavailable)"
    }`,
    `CLI: ${tildify(cli)}${
      cliVersion ? ` (${cliVersion})` : " (version unavailable)"
    }`,
    `workspace trust: ${vscode.workspace.isTrusted ? "trusted" : "untrusted"}`,
    `workspace folders: ${
      folders.length > 0
        ? folders.map((folder) => tildify(folder.uri.fsPath)).join(", ")
        : "none"
    }`,
    `type-aware diagnostics: ${settings.typeAware ? "on" : "off"} · scope: ${
      settings.tscScope
    } · debounce: ${settings.debounceMs}ms`,
    `syntax diagnostics: ${settings.syntaxDiagnostics ? "on" : "off"}`,
    `fixes on save: ${settings.fixesOnSave}`,
    `TypeScript SDK: ${
      settings.tsdk ? tildify(expandHome(settings.tsdk)) : "auto"
    }`,
    `tsc compiler: ${
      settings.tsdk
        ? "set above; compiler/version is not reported by the server"
        : "auto-discovered by the server; compiler/version is not reported"
    }`,
  ];
  try {
    const result = await explain(client, target.uri, target.languageId);
    if (result.error) {
      lines.push(`configuration: error: ${result.error}`);
    } else {
      lines.push(
        `language: ${result.language ?? "unmapped"}${
          result.dialect ? ` (${result.dialect})` : ""
        }${result.skipped ? ` · skipped: ${result.skipped}` : ""}`,
      );
      lines.push("configuration:");
      if (result.configs.length === 0) {
        lines.push("  (none)");
      } else {
        for (const file of result.configs) {
          lines.push(`  ${file}`);
        }
      }
    }
  } catch (error) {
    lines.push(`configuration: unavailable (${message(error)})`);
  }
  return lines.join("\n");
}

function doctorTarget(): { uri: vscode.Uri; languageId: string } {
  const document = supportedDocument();
  if (document) {
    return { uri: document.uri, languageId: document.languageId };
  }
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (folder) {
    return {
      uri: vscode.Uri.joinPath(folder.uri, "__dotfmt_doctor__.ts"),
      languageId: "typescript",
    };
  }
  return {
    uri: vscode.Uri.file(
      path.join(path.dirname(globalConfigFile()), "__dotfmt_doctor__.ts"),
    ),
    languageId: "typescript",
  };
}

function supportedDocument(): vscode.TextDocument | undefined {
  const document = vscode.window.activeTextEditor?.document;
  return document && vscode.languages.match(SELECTOR, document) > 0
    ? document
    : undefined;
}
