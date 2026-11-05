import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import * as vscode from "vscode";
import {
  LanguageClient,
  RevealOutputChannelOn,
  TransportKind,
  type ExecuteCommandSignature,
  type LanguageClientOptions,
  type ServerOptions,
} from "vscode-languageclient/node";
import { CONFIG_NAME, SECTION, SELECTOR } from "./lib/languages";
import { expandHome, tildify } from "./lib/paths";
import { readSettings } from "./lib/settings";

const execFileAsync = promisify(execFile);

const DOCUMENT_COMMANDS = new Set([
  "dotfmt.fixAll",
  "dotfmt.organizeImports",
  "dotfmt.sortTailwind",
]);

/** Thin wrapper around one language client, restarted when its binary or TypeScript SDK moves. */
export class Client implements vscode.Disposable {
  private client: LanguageClient | undefined;
  private ready: Promise<void> = Promise.resolve();
  private restarting: Promise<void> | undefined;
  private serverVersion: string | undefined;

  public constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly log: vscode.LogOutputChannel,
    private readonly trace: vscode.LogOutputChannel,
  ) {}

  public serverPath(): string {
    const configured = vscode.workspace
      .getConfiguration(SECTION)
      .get<string>("server.path", "")
      .trim();
    return configured
      ? expandHome(configured)
      : this.context.asAbsolutePath(
          path.join(
            "bin",
            `${process.platform}-${process.arch}`,
            executable("dotfmt-lsp"),
          ),
        );
  }

  /** The `dotfmt-lsp` version reported during initialize, once the server is running. */
  public version(): string | undefined {
    return this.serverVersion;
  }

  public start(): Promise<void> {
    const command = this.serverPath();
    const folder = vscode.workspace.workspaceFolders?.[0];
    const settings = readSettings(folder?.uri);
    const tsdk = settings.tsdk ? expandHome(settings.tsdk) : undefined;
    const server: ServerOptions = {
      command,
      transport: TransportKind.stdio,
      options: {
        env: {
          ...process.env,
          DOTFMT_TRUSTED: vscode.workspace.isTrusted ? "1" : "0",
        },
      },
    };
    const documentSelector: vscode.DocumentFilter[] = [];
    for (const filter of SELECTOR) {
      if (filter.language) {
        documentSelector.push({
          scheme: filter.scheme,
          language: filter.language,
        });
      } else if (typeof filter.pattern === "string") {
        documentSelector.push({
          scheme: filter.scheme,
          pattern: filter.pattern,
        });
      }
    }
    const options: LanguageClientOptions = {
      documentSelector:
        documentSelector as unknown as LanguageClientOptions["documentSelector"],
      diagnosticCollectionName: "dotfmt",
      outputChannel: this.log,
      traceOutputChannel: this.trace,
      revealOutputChannelOn: RevealOutputChannelOn.Never,
      initializationOptions: {
        trusted: vscode.workspace.isTrusted,
        globalConfig: globalConfigFile(),
        ...(tsdk ? { tsdk } : {}),
      },
      middleware: {
        executeCommand: (executable, args, next) =>
          this.execute(executable, args, next),
      },
      synchronize: {
        configurationSection: SECTION,
        fileEvents: [
          vscode.workspace.createFileSystemWatcher(`**/${CONFIG_NAME}`),
          vscode.workspace.createFileSystemWatcher(
            new vscode.RelativePattern(
              vscode.Uri.file(globalConfigDirectory()),
              CONFIG_NAME,
            ),
          ),
          vscode.workspace.createFileSystemWatcher("**/.prettierignore"),
          vscode.workspace.createFileSystemWatcher("**/tsconfig*.json"),
          vscode.workspace.createFileSystemWatcher("**/jsconfig*.json"),
        ],
      },
    };
    const client = new LanguageClient(SECTION, "dotfmt", server, options);
    this.client = client;
    this.serverVersion = undefined;
    this.log.info(`server: ${tildify(command)}`);
    this.ready = client.start().then(() => {
      this.serverVersion = client.initializeResult?.serverInfo?.version;
      if (this.serverVersion) {
        this.log.info(`dotfmt-lsp version: ${this.serverVersion}`);
      }
    });
    return this.ready;
  }

  public async request<T>(method: string, params: unknown): Promise<T> {
    await this.ready;
    if (!this.client) {
      throw new Error("dotfmt language server is not running");
    }
    return this.client.sendRequest<T>(method, params);
  }

  private async execute(
    command: string,
    args: unknown[],
    next: ExecuteCommandSignature,
  ): Promise<unknown> {
    if (args.length === 0 && DOCUMENT_COMMANDS.has(command)) {
      const document = vscode.window.activeTextEditor?.document;
      if (!document || vscode.languages.match(SELECTOR, document) === 0) {
        return undefined;
      }
      args = [document.uri.toString()];
    }
    try {
      return await next(command, args);
    } catch (error) {
      void vscode.window.showErrorMessage(`dotfmt: ${message(error)}`);
      return undefined;
    }
  }

  public stop(): Promise<void> {
    const client = this.client;
    this.client = undefined;
    this.serverVersion = undefined;
    if (!client) {
      return Promise.resolve();
    }
    return client.stop().catch((error: unknown) => {
      this.log.error(`server stop failed: ${message(error)}`);
    });
  }

  public restart(): Promise<void> {
    if (this.restarting) {
      return this.restarting;
    }
    this.restarting = (async () => {
      try {
        // Let an in-flight start finish first; stopping mid-start can leave a
        // client that the next start overwrites without shutting down.
        await this.ready.catch(() => undefined);
        await this.stop();
        await this.start();
      } finally {
        this.restarting = undefined;
      }
    })();
    return this.restarting;
  }

  public dispose(): void {
    void this.stop();
  }
}

/** The extension's package.json version. */
export function extensionVersion(context: vscode.ExtensionContext): string {
  const version = (context.extension.packageJSON as { version?: unknown })
    .version;
  return typeof version === "string" ? version : "unknown";
}

export function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The configured or bundled dotfmt CLI, kept for status, doctor and manual checks. */
export function cliPath(context: vscode.ExtensionContext): string {
  const configured = vscode.workspace
    .getConfiguration(SECTION)
    .get<string>("path", "")
    .trim();
  if (configured) {
    return expandHome(configured);
  }
  const bundled = context.asAbsolutePath(
    path.join(
      "bin",
      `${process.platform}-${process.arch}`,
      executable("dotfmt"),
    ),
  );
  return existsSync(bundled) ? bundled : "dotfmt";
}

/** The first line of `<command> --version`, or undefined when the binary is missing. */
export async function binaryVersion(
  command: string,
): Promise<string | undefined> {
  try {
    const { stdout } = await execFileAsync(command, ["--version"], {
      timeout: 5000,
      encoding: "utf8",
    });
    return stdout.trim().split(/\r?\n/)[0]?.trim() || undefined;
  } catch {
    return undefined;
  }
}

export function globalConfigDirectory(): string {
  return path.join(
    process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"),
    "dotfmt",
  );
}

export function globalConfigFile(): string {
  return path.join(globalConfigDirectory(), CONFIG_NAME);
}

function executable(name: string): string {
  return process.platform === "win32" ? `${name}.exe` : name;
}
