import * as vscode from "vscode";
import { cliPath, message, type Client } from "./client";
import {
  projectForFile,
  requestDoctor,
  tscState,
  type DoctorReport,
} from "./lib/doctor";
import { explain, type Explanation } from "./lib/explain";
import { CONFIG_NAME, SECTION, SELECTOR } from "./lib/languages";
import { expandHome, tildify } from "./lib/paths";
import { readSettings } from "./lib/settings";

type Result =
  | { ok: true; value: Explanation; report?: DoctorReport }
  | { ok: false; failure: string };

export class Status implements vscode.Disposable {
  private readonly item = vscode.languages.createLanguageStatusItem(
    SECTION,
    SELECTOR,
  );
  private readonly cache = new Map<string, Result>();
  private generation = 0;

  public constructor(
    private readonly client: Client,
    private readonly context: vscode.ExtensionContext,
    private readonly version: string,
  ) {
    this.item.name = "dotfmt";
    this.item.text = "dotfmt";
  }

  public invalidate(): void {
    this.cache.clear();
    void this.refresh();
  }

  public async refresh(): Promise<void> {
    const generation = ++this.generation;
    const document = vscode.window.activeTextEditor?.document;
    if (!document || vscode.languages.match(SELECTOR, document) === 0) {
      if (generation === this.generation) {
        this.clear();
      }
      return;
    }
    const key = `${document.languageId}:${document.uri.toString()}`;
    let result = this.cache.get(key);
    if (!result) {
      this.item.busy = true;
      result = await explainDocument(this.client, document);
      this.cache.set(key, result);
    }
    if (generation === this.generation) {
      this.item.busy = false;
      this.show(result, document);
    }
  }

  public dispose(): void {
    this.item.dispose();
  }

  private clear(): void {
    this.item.busy = false;
    this.item.severity = vscode.LanguageStatusSeverity.Information;
    this.item.detail = undefined;
    this.item.command = undefined;
  }

  private show(result: Result, document: vscode.TextDocument): void {
    if (!result.ok) {
      this.fail(result.failure);
      return;
    }
    const { language, dialect, skipped, configs } = result.value;
    const nearest = configs.at(-1);
    this.item.severity = vscode.LanguageStatusSeverity.Information;
    this.item.detail = [
      language && dialect ? `${language} (${dialect})` : language,
      skipped,
    ]
      .filter(Boolean)
      .join(" ");
    const tooltip = this.tooltip(document, configs, result.report);
    this.item.command = nearest
      ? {
          title: CONFIG_NAME,
          command: "vscode.open",
          arguments: [vscode.Uri.file(nearest)],
          tooltip,
        }
      : {
          title: "Open Configuration",
          command: "dotfmt.openConfig",
          tooltip,
        };
  }

  private tooltip(
    document: vscode.TextDocument,
    configs: string[],
    report?: DoctorReport,
  ): string {
    const settings = readSettings(document.uri);
    const project = report
      ? projectForFile(report, document.uri.fsPath)
      : undefined;
    const serverVersion = report?.server.version ?? this.client.version();
    const serverName = report?.server.name ?? "dotfmt-lsp";
    const tsc = tscState(
      vscode.workspace.isTrusted,
      settings.typeAware,
      settings.tscScope,
    );
    const lines = [
      `dotfmt ${this.version}`,
      `server: ${tildify(this.client.serverPath())}${
        serverVersion ? ` (${serverName} ${serverVersion})` : ""
      }`,
      `CLI: ${tildify(cliPath(this.context))}`,
      `tsc: ${tsc}${project?.state ? ` · ${project.state}` : ""}`,
      ...(project?.compiler
        ? [
            `compiler: ${tildify(project.compiler)}${
              project.version ? ` (${project.version})` : ""
            }`,
          ]
        : []),
      `fixes on save: ${settings.fixesOnSave}`,
      `syntax diagnostics: ${settings.syntaxDiagnostics ? "on" : "off"}`,
      `TypeScript SDK: ${
        settings.tsdk ? tildify(expandHome(settings.tsdk)) : "auto"
      }`,
    ];
    if (configs.length > 0) {
      lines.push(
        "configuration:",
        ...configs.map((file) => `  ${tildify(file)}`),
      );
    }
    return lines.join("\n");
  }

  private fail(failure: string): void {
    const serverVersion = this.client.version();
    this.item.severity = vscode.LanguageStatusSeverity.Error;
    this.item.detail = failure;
    this.item.command = {
      title: "Show Output",
      command: "dotfmt.showOutput",
      tooltip: `dotfmt ${this.version}\nserver: ${tildify(
        this.client.serverPath(),
      )}${serverVersion ? ` (dotfmt-lsp ${serverVersion})` : ""}\nCLI: ${tildify(
        cliPath(this.context),
      )}`,
    };
  }
}

async function explainDocument(
  client: Client,
  document: vscode.TextDocument,
): Promise<Result> {
  try {
    const [value, known] = await Promise.all([
      explain(client, document.uri, document.languageId),
      requestDoctor(client),
    ]);
    return value.error
      ? { ok: false, failure: value.error }
      : { ok: true, value, report: known.report };
  } catch (error) {
    return { ok: false, failure: message(error) };
  }
}
