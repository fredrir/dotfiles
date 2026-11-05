import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  RevealOutputChannelOn,
  TransportKind,
  type LanguageClientOptions,
  type ServerOptions,
} from "vscode-languageclient/node";
import { CONFIG_NAME, SECTION, SELECTOR } from "./lib/languages";

/** Thin wrapper around one language client, restarted when its binary moves. */
export class Client implements vscode.Disposable {
  private client: LanguageClient | undefined;
  private ready: Promise<void> = Promise.resolve();
  private restarting: Promise<void> | undefined;

  public constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly log: vscode.LogOutputChannel,
  ) {}

  public serverPath(): string {
    const configured = vscode.workspace
      .getConfiguration(SECTION)
      .get<string>("server.path", "")
      .trim();
    return configured
      ? expand(configured)
      : this.context.asAbsolutePath(
          path.join(
            "bin",
            `${process.platform}-${process.arch}`,
            executable("dotfmt-lsp"),
          ),
        );
  }

  public start(): Promise<void> {
    const server: ServerOptions = {
      command: this.serverPath(),
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
        documentSelector.push({ scheme: filter.scheme, language: filter.language });
      } else if (typeof filter.pattern === "string") {
        documentSelector.push({ scheme: filter.scheme, pattern: filter.pattern });
      }
    }
    const options: LanguageClientOptions = {
      documentSelector: documentSelector as unknown as LanguageClientOptions["documentSelector"],
      diagnosticCollectionName: "dotfmt",
      outputChannel: this.log,
      revealOutputChannelOn: RevealOutputChannelOn.Never,
      initializationOptions: { trusted: vscode.workspace.isTrusted },
      synchronize: {
        configurationSection: SECTION,
        fileEvents: [
          vscode.workspace.createFileSystemWatcher(`**/${CONFIG_NAME}`),
          vscode.workspace.createFileSystemWatcher(
            new vscode.RelativePattern(globalConfig(), CONFIG_NAME),
          ),
          vscode.workspace.createFileSystemWatcher("**/tsconfig.json"),
        ],
      },
    };
    this.client = new LanguageClient(SECTION, "dotfmt", server, options);
    this.ready = this.client.start();
    return this.ready;
  }

  public async request<T>(method: string, params: unknown): Promise<T> {
    await this.ready;
    if (!this.client) {
      throw new Error("dotfmt language server is not running");
    }
    return this.client.sendRequest<T>(method, params);
  }

  public stop(): Promise<void> {
    const client = this.client;
    this.client = undefined;
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

export function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function executable(name: string): string {
  return process.platform === "win32" ? `${name}.exe` : name;
}

function expand(value: string): string {
  return value.replace(/^~(?=$|[\\/])/, os.homedir());
}

function globalConfig(): vscode.Uri {
  return vscode.Uri.file(
    path.join(
      process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"),
      "dotfmt",
    ),
  );
}

/** The configured dotfmt CLI, kept for status and manual checks. */
export function cliPath(context: vscode.ExtensionContext): string {
  const configured = vscode.workspace
    .getConfiguration(SECTION)
    .get<string>("path", "")
    .trim();
  if (configured) {
    return expand(configured);
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
