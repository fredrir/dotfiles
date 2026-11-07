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
import {
  doctorReportLines,
  projectForFile,
  requestDoctor,
  tscState,
} from "./lib/doctor";
import { explain, type Explanation } from "./lib/explain";
import {
  CONFIG_NAME,
  enabledFormatterLanguages,
  SELECTOR,
} from "./lib/languages";
import { configTarget, formatSettingsQuery } from "./lib/native";
import { expandHome, findConfig, isWithin, tildify } from "./lib/paths";
import { readSettings } from "./lib/settings";
import type { Status } from "./status";

export function registerCommands(
  context: vscode.ExtensionContext,
  client: Client,
  log: vscode.LogOutputChannel,
  status: Status,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dotfmt.restartServer", async () => {
      log.info("restarting language server");
      try {
        await client.restart();
      } catch (error) {
        log.error(`server restart failed: ${message(error)}`);
        void vscode.window.showErrorMessage(
          `dotfmt: language server failed to restart: ${message(error)}`,
        );
      } finally {
        status.invalidate();
      }
    }),
    vscode.commands.registerCommand("dotfmt.showOutput", () => log.show(true)),
    vscode.commands.registerCommand("dotfmt.setAsDefaultFormatter", () =>
      setAsDefaultFormatter(context, client),
    ),
    vscode.commands.registerCommand("dotfmt.configure", () =>
      vscode.commands.executeCommand(
        "workbench.action.openSettings",
        formatSettingsQuery(),
      ),
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
  client: Client,
): Promise<void> {
  const hasFolder = (vscode.workspace.workspaceFolders?.length ?? 0) > 0;
  const target = hasFolder
    ? vscode.ConfigurationTarget.Workspace
    : vscode.ConfigurationTarget.Global;
  const folder =
    (vscode.window.activeTextEditor
      ? vscode.workspace.getWorkspaceFolder(
          vscode.window.activeTextEditor.document.uri,
        )
      : undefined) ?? vscode.workspace.workspaceFolders?.[0];
  const available = await vscode.languages.getLanguages();
  let languages: string[];
  try {
    const response = await client.request<unknown>(
      "dotfmt/languages",
      folder ? { scopeUri: folder.uri.toString() } : {},
    );
    const names = languageNames(response);
    if (!names) {
      throw new Error("unexpected dotfmt/languages response");
    }
    languages = enabledFormatterLanguages(names, available);
  } catch (error) {
    void vscode.window.showErrorMessage(
      `dotfmt: cannot read the enabled languages (${message(error)}); no formatter settings were changed.`,
    );
    return;
  }
  if (languages.length === 0) {
    void vscode.window.showWarningMessage(
      "dotfmt: no enabled languages to set as the default formatter.",
    );
    return;
  }
  const failures: string[] = [];
  await Promise.all(
    languages.map(async (language) => {
      try {
        await vscode.workspace
          .getConfiguration("editor", { languageId: language })
          .update("defaultFormatter", context.extension.id, target, true);
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
    `dotfmt is now the default formatter for its enabled languages in ${
      hasFolder ? "this workspace" : "user settings"
    }.`,
  );
}

function languageNames(value: unknown): string[] | undefined {
  if (Array.isArray(value)) {
    return value.filter((item): item is string => typeof item === "string");
  }
  if (typeof value === "object" && value !== null) {
    const languages = (value as { languages?: unknown }).languages;
    if (Array.isArray(languages)) {
      return languages.filter(
        (item): item is string => typeof item === "string",
      );
    }
  }
  return undefined;
}

async function openConfig(): Promise<void> {
  const document = vscode.window.activeTextEditor?.document;
  const folder =
    (document
      ? vscode.workspace.getWorkspaceFolder(document.uri)
      : undefined) ?? vscode.workspace.workspaceFolders?.[0];
  const local = folder ? localConfig(document, folder.uri.fsPath) : undefined;
  const target = configTarget(local, globalConfigFile(), existsSync);
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
    const [result, known] = await Promise.all([
      explain(client, document.uri, document.languageId),
      requestDoctor(client),
    ]);
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
    const settings = readSettings(document.uri);
    const report = known.report;
    const project = report
      ? projectForFile(report, document.uri.fsPath)
      : undefined;
    const server = report?.server;
    const serverVersion = server?.version ?? client.version();
    const tsc = tscState(
      vscode.workspace.isTrusted,
      settings.typeAware,
      settings.tscScope,
    );
    const summary = [
      detail,
      `tsc: ${tsc}${project?.state ? `, ${project.state}` : ""}`,
      serverVersion
        ? `server: ${server?.name ?? "dotfmt-lsp"} ${serverVersion}`
        : undefined,
      nearest ? tildify(nearest) : undefined,
    ]
      .filter(Boolean)
      .join(" · ");
    const doctor = "Doctor";
    const answer = await vscode.window.showInformationMessage(
      `dotfmt: ${summary}`,
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
  const folders = vscode.workspace.workspaceFolders ?? [];
  const target = doctorTarget();
  const settings = readSettings(target.uri);
  const [cliVersion, known, result] = await Promise.all([
    binaryVersion(cli),
    requestDoctor(client),
    explain(client, target.uri, target.languageId).catch(
      (error): Explanation => ({ error: message(error), configs: [] }),
    ),
  ]);
  const report = known.report;
  const serverVersion = report?.server.version ?? client.version();
  const serverName = report?.server.name ?? "dotfmt-lsp";
  const lines = [
    "dotfmt doctor",
    `extension: ${context.extension.id} ${version}`,
    `server: ${tildify(client.serverPath())}${
      serverVersion
        ? ` (${serverName} ${serverVersion})`
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
  ];
  if (report) {
    lines.push(...doctorReportLines(report, tildify));
  } else {
    lines.push(
      `server report: unavailable (${known.error ?? "unexpected response"})`,
      `type-aware diagnostics: ${settings.typeAware ? "on" : "off"} · scope: ${
        settings.tscScope
      } · debounce: ${settings.debounceMs}ms`,
      `syntax diagnostics: ${settings.syntaxDiagnostics ? "on" : "off"}`,
      "tsc compiler: unavailable (the server did not report it)",
    );
  }
  lines.push(
    `fixes on save: ${settings.fixesOnSave}`,
    `TypeScript SDK: ${
      settings.tsdk ? tildify(expandHome(settings.tsdk)) : "auto"
    }`,
  );
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
        lines.push(`  ${tildify(file)}`);
      }
    }
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
