import * as vscode from "vscode";
import { range, type Failure, type Location } from "./failure";

export class Reporter implements vscode.Disposable {
  private readonly diagnostics =
    vscode.languages.createDiagnosticCollection("dotfmt");
  private readonly notified = new Map<string, string>();

  public constructor(private readonly log: vscode.LogOutputChannel) {}

  /** Notifications are for formatting the user asked for; passive checks only mark Problems. */
  public failed(key: string, failure: Failure, notify: boolean): void {
    this.log.error(failure.log);
    for (const [file, located] of Map.groupBy(
      failure.locations,
      (location) => location.file,
    )) {
      this.diagnostics.set(vscode.Uri.file(file), located.map(diagnostic));
    }
    if (notify && !this.notified.has(failure.message)) {
      this.notified.set(failure.message, key);
      void this.notify(failure);
    }
  }

  public succeeded(key: string): void {
    for (const [message, owner] of this.notified) {
      if (owner === key) {
        this.notified.delete(message);
      }
    }
  }

  public validated(configs: readonly string[]): void {
    for (const config of configs) {
      this.diagnostics.delete(vscode.Uri.file(config));
    }
  }

  public forget(uri: vscode.Uri): void {
    this.diagnostics.delete(uri);
  }

  public dispose(): void {
    this.diagnostics.dispose();
  }

  private async notify(failure: Failure): Promise<void> {
    const location = failure.locations[0];
    const action = location ? "Open" : "Show Output";
    if (
      (await vscode.window.showErrorMessage(failure.message, action)) !== action
    ) {
      return;
    }
    if (location) {
      await vscode.window.showTextDocument(vscode.Uri.file(location.file), {
        selection: range(location),
      });
    } else {
      this.log.show(true);
    }
  }
}

function diagnostic(location: Location): vscode.Diagnostic {
  const result = new vscode.Diagnostic(
    range(location),
    location.message,
    vscode.DiagnosticSeverity.Error,
  );
  result.source = "dotfmt";
  return result;
}
