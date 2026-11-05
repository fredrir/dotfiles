import * as vscode from "vscode";
import { message, type Client } from "./client";
import { CONFIG_NAME, SECTION, SELECTOR } from "./lib/languages";

interface Explanation {
  language?: string;
  dialect?: string;
  skipped?: string;
  configs: string[];
  error?: string;
}

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
    private readonly cli: string,
  ) {
    this.item.name = "dotfmt";
    this.item.text = "dotfmt";
  }

  public invalidate(): void {
    this.cache.clear();
    void this.refresh();
  }

  public async refresh(): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (!document || vscode.languages.match(SELECTOR, document) === 0) {
      return;
    }
    const generation = ++this.generation;
    const key = `${document.languageId}:${document.uri.toString()}`;
    let result = this.cache.get(key);
    if (!result) {
      this.item.busy = true;
      result = await this.explain(document);
      this.cache.set(key, result);
    }
    if (generation === this.generation) {
      this.item.busy = false;
      this.show(result);
    }
  }

  public dispose(): void {
    this.item.dispose();
  }

  private async explain(document: vscode.TextDocument): Promise<Result> {
    try {
      const value = await this.client.request<Explanation>("dotfmt/explain", {
        textDocument: { uri: document.uri.toString() },
        languageId: document.languageId,
      });
      return value.error
        ? { ok: false, failure: value.error }
        : { ok: true, value };
    } catch (error) {
      return { ok: false, failure: message(error) };
    }
  }

  private show(result: Result): void {
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
    this.item.command = nearest
      ? {
          title: CONFIG_NAME,
          command: "vscode.open",
          arguments: [vscode.Uri.file(nearest)],
          tooltip: [
            ...configs.map(tildify),
            `dotfmt: ${tildify(this.cli)}`,
          ].join("\n"),
        }
      : undefined;
  }

  private fail(failure: string): void {
    this.item.severity = vscode.LanguageStatusSeverity.Error;
    this.item.detail = failure;
    this.item.command = {
      title: "Show Output",
      command: "dotfmt.showOutput",
    };
  }
}

function tildify(file: string): string {
  const home = process.env.HOME;
  return home && (file === home || file.startsWith(`${home}/`))
    ? `~${file.slice(home.length)}`
    : file;
}
