import * as vscode from "vscode";
import type { Dotfmt, Explanation, Result } from "./dotfmt";
import { range, tildify, type Failure } from "./failure";
import { CONFIG_NAME, SECTION, SELECTOR } from "./lib/languages";

export class Status implements vscode.Disposable {
  private readonly item = vscode.languages.createLanguageStatusItem(
    SECTION,
    SELECTOR,
  );
  private readonly cache = new Map<string, Result<Explanation>>();
  private generation = 0;

  public constructor(private readonly dotfmt: Dotfmt) {
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
      result = await this.dotfmt.explain(document);
      this.cache.set(key, result);
    }
    if (generation === this.generation) {
      this.item.busy = false;
      this.show(result);
    }
  }

  public formatted(document: vscode.TextDocument, failure?: Failure): void {
    if (document !== vscode.window.activeTextEditor?.document) {
      return;
    }
    if (failure) {
      this.generation++;
      this.item.busy = false;
      this.fail(failure);
    } else if (this.item.severity === vscode.LanguageStatusSeverity.Error) {
      void this.refresh();
    }
  }

  public dispose(): void {
    this.item.dispose();
  }

  private show(result: Result<Explanation>): void {
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
          tooltip: configs.map(tildify).join("\n"),
        }
      : undefined;
  }

  private fail(failure: Failure): void {
    const location = failure.locations[0];
    this.item.severity = vscode.LanguageStatusSeverity.Error;
    this.item.detail = failure.message;
    this.item.command = location
      ? {
          title: "Open",
          command: "vscode.open",
          arguments: [
            vscode.Uri.file(location.file),
            { selection: range(location) },
          ],
        }
      : { title: "Show Output", command: "dotfmt.showOutput" };
  }
}
