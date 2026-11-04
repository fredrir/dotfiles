import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import * as vscode from "vscode";
import { failure, type Failure } from "./failure";
import { LANGUAGES, SECTION } from "./lib/languages";
import { run, type Run } from "./lib/process";
import type { Reporter } from "./report";

export interface Explanation {
  language?: string;
  dialect?: string;
  skipped?: string;
  configs: string[];
}

export type Result<T> =
  { ok: true; value: T } | { ok: false; failure: Failure };

export interface Formatted {
  document: vscode.TextDocument;
  failure?: Failure;
}

interface Target {
  cwd: string;
  filename: string;
  args: string[];
}

export class Dotfmt implements vscode.Disposable {
  private readonly formatted = new vscode.EventEmitter<Formatted>();
  public readonly onDidFormat = this.formatted.event;

  public constructor(
    private readonly extensionPath: string,
    private readonly log: vscode.LogOutputChannel,
    private readonly reporter: Reporter,
  ) {}

  /** Resolves to the formatted text, or undefined when there is nothing to apply. */
  public async format(
    document: vscode.TextDocument,
    input: string,
    token: vscode.CancellationToken,
  ): Promise<string | undefined> {
    const target = this.target(document);
    if (!target) {
      return undefined;
    }
    const result = await this.invoke(
      [...target.args, "--editor", target.filename],
      target.cwd,
      input,
      token,
    );
    const key = document.uri.toString();
    switch (result.kind) {
      case "cancelled":
        return undefined;
      case "ok":
        this.reporter.succeeded(key);
        this.formatted.fire({ document });
        return result.stdout;
      default: {
        const failed = failure(result, this.binary());
        this.reporter.failed(key, failed, true);
        this.formatted.fire({ document, failure: failed });
        return undefined;
      }
    }
  }

  public explain(document: vscode.TextDocument): Promise<Result<Explanation>> {
    const target = this.target(document);
    return target
      ? this.explainTarget(target)
      : Promise.resolve({
          ok: true,
          value: { skipped: "unmapped", configs: [] },
        });
  }

  /** Explaining a config file validates it together with its ancestors. */
  public async validate(config: vscode.Uri): Promise<void> {
    const result = await this.explainTarget(fileTarget(config));
    if (
      !result.ok &&
      !result.failure.locations.some(
        (location) =>
          vscode.Uri.file(location.file).toString() === config.toString(),
      )
    ) {
      this.reporter.forget(config);
    }
  }

  public dispose(): void {
    this.formatted.dispose();
  }

  private binary(): string {
    const configured = vscode.workspace
      .getConfiguration(SECTION)
      .get<string>("path", "")
      .trim();
    return configured
      ? configured.replace(/^~(?=$|[\\/])/, os.homedir())
      : path.join(
          this.extensionPath,
          "bin",
          `${process.platform}-${process.arch}`,
          "dotfmt",
        );
  }

  private target(document: vscode.TextDocument): Target | undefined {
    const { uri } = document;
    if (uri.scheme === "file") {
      return fileTarget(uri);
    }
    const mapped =
      uri.scheme === "untitled" ? LANGUAGES[document.languageId] : undefined;
    if (!mapped) {
      return undefined;
    }
    const [language, dialect] = mapped;
    return {
      cwd: directory(vscode.workspace.workspaceFolders?.[0]?.uri.fsPath),
      filename: path.basename(uri.path) || "Untitled",
      args: ["--lang", language, ...(dialect ? ["--dialect", dialect] : [])],
    };
  }

  private async explainTarget(target: Target): Promise<Result<Explanation>> {
    const result = await this.invoke(
      [...target.args, "--explain", target.filename],
      target.cwd,
    );
    switch (result.kind) {
      case "ok": {
        const explanation = parse(result.stdout);
        this.reporter.validated(explanation.configs);
        return { ok: true, value: explanation };
      }
      case "cancelled":
        return { ok: true, value: { configs: [] } };
      default: {
        const failed = failure(result, this.binary());
        this.reporter.failed(target.filename, failed, false);
        return { ok: false, failure: failed };
      }
    }
  }

  private async invoke(
    args: string[],
    cwd: string,
    input?: string,
    token?: vscode.CancellationToken,
  ): Promise<Run> {
    const binary = this.binary();
    this.log.trace(`${binary} ${args.join(" ")} (in ${cwd})`);
    const started = performance.now();
    const result = await run({ binary, args, cwd, input, token });
    this.log.debug(
      `${args.join(" ")}: ${result.kind} in ${Math.round(performance.now() - started)} ms`,
    );
    return result;
  }
}

function fileTarget(uri: vscode.Uri): Target {
  return {
    cwd: directory(
      vscode.workspace.getWorkspaceFolder(uri)?.uri.fsPath,
      path.dirname(uri.fsPath),
    ),
    filename: uri.fsPath,
    args: [],
  };
}

function directory(...candidates: (string | undefined)[]): string {
  return (
    candidates.find(
      (candidate): candidate is string =>
        candidate !== undefined && existsSync(candidate),
    ) ?? os.homedir()
  );
}

function parse(stdout: string): Explanation {
  const explanation: Explanation = { configs: [] };
  for (const line of stdout.split(/\r?\n/)) {
    const [, key, value] = /^(\w+)\s+(.+)$/.exec(line) ?? [];
    if (!value) {
      continue;
    }
    if (key === "config") {
      explanation.configs.push(value);
    } else if (key === "language" || key === "dialect" || key === "skipped") {
      explanation[key] = value;
    }
  }
  return explanation;
}
