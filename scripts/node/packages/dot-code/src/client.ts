import path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  TransportKind,
  type LanguageClientOptions,
} from "vscode-languageclient/node";
import { LANGUAGES, SECTION } from "./lib/constants";

export class ClientManager {
  private client: LanguageClient | undefined;
  private restartPromise: Promise<void> = Promise.resolve();
  private isRestarting = false;
  private manualShutdown = false;

  public constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly outputChannel: vscode.LogOutputChannel,
  ) {}

  public serverPath(): string {
    const configured = vscode.workspace
      .getConfiguration(SECTION)
      .get<string>("server.path");
    return (
      configured ||
      this.context.asAbsolutePath(
        path.join(
          "bin",
          `${process.platform}-${process.arch}`,
          "dot-code-server",
        ),
      )
    );
  }

  public async start(): Promise<void> {
    this.manualShutdown = false;
    const options: LanguageClientOptions = {
      documentSelector: LANGUAGES.flatMap((language) => [
        { scheme: "file", language },
        { scheme: "untitled", language },
      ]),
      synchronize: { configurationSection: SECTION },
      outputChannel: this.outputChannel,
    };
    this.client = new LanguageClient(
      SECTION,
      "Dot Code",
      { command: this.serverPath(), transport: TransportKind.stdio },
      options,
    );
    await this.client.start();
  }

  public async stop(): Promise<void> {
    this.manualShutdown = true;
    const currentClient = this.client;
    this.client = undefined;

    if (currentClient) {
      try {
        if (currentClient.isRunning()) {
          await currentClient.stop();
        }
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        this.outputChannel.error(`server stop failed: ${msg}`);
      }
    }
  }

  public restart(): Promise<void> {
    if (this.isRestarting) {
      return this.restartPromise;
    }

    this.isRestarting = true;
    this.restartPromise = (async () => {
      try {
        await this.stop();
        await this.start();
      } finally {
        this.isRestarting = false;
      }
    })();

    return this.restartPromise;
  }
}
