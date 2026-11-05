import * as vscode from "vscode";
import { cliPath, message, type Client } from "./client";
import { explain, type Explanation } from "./lib/explain";
import { CONFIG_NAME, SECTION, SELECTOR } from "./lib/languages";
import { expandHome, tildify } from "./lib/paths";
import { readSettings } from "./lib/settings";

type Result = { ok: true; value: Explanation } | { ok: false; failure: string };

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
    const tooltip = this.tooltip(document, configs);
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

  private tooltip(document: vscode.TextDocument, configs: string[]): string {
    const settings = readSettings(document.uri);
    const serverVersion = this.client.version();
    const lines = [
      `dotfmt ${this.version}`,
      `server: ${tildify(this.client.serverPath())}${
        serverVersion ? ` (dotfmt-lsp ${serverVersion})` : ""
      }`,
      `CLI: ${tildify(cliPath(this.context))}`,
      `tsc: ${typescriptState(settings.typeAware, settings.tscScope)}`,
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
    this.item.severity = vscode.LanguageStatusSeverity.Error;
    this.item.detail = failure;
    this.item.command = {
      title: "Show Output",
      command: "dotfmt.showOutput",
      tooltip: `dotfmt ${this.version}\nserver: ${tildify(
        this.client.serverPath(),
      )}\nCLI: ${tildify(cliPath(this.context))}`,
    };
  }
}

async function explainDocument(
  client: Client,
  document: vscode.TextDocument,
): Promise<Result> {
  try {
    const value = await explain(client, document.uri, document.languageId);
    return value.error
      ? { ok: false, failure: value.error }
      : { ok: true, value };
  } catch (error) {
    return { ok: false, failure: message(error) };
  }
}

function typescriptState(
  typeAware: boolean,
  scope: "open" | "project",
): string {
  if (!vscode.workspace.isTrusted) {
    return "off (untrusted)";
  }
  return typeAware ? `on (${scope})` : "off";
}
