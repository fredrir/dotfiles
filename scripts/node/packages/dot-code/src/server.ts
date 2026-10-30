import type * as vscode from "vscode";
import type * as Client from "./client";

export class Server {
  private readonly manager: Promise<Client.ClientManager>;

  public constructor(
    context: vscode.ExtensionContext,
    outputChannel: vscode.LogOutputChannel,
  ) {
    this.manager = new Promise((resolve, reject) => {
      setImmediate(() => {
        try {
          // oxlint-disable-next-line typescript/no-require-imports -- separate bundle, loaded after activation
          const client = require("./client") as typeof Client;
          const manager = new client.ClientManager(context, outputChannel);
          manager.start().catch((error: unknown) => {
            const msg = error instanceof Error ? error.message : String(error);
            outputChannel.error(`server start failed: ${msg}`);
          });
          resolve(manager);
        } catch (error) {
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      });
    });
    this.manager.catch((error: unknown) => {
      const msg = error instanceof Error ? error.message : String(error);
      outputChannel.error(`client load failed: ${msg}`);
    });
  }

  public async request<R>(method: string, params: unknown): Promise<R> {
    return (await this.manager).request<R>(method, params);
  }

  public async restart(): Promise<void> {
    await (await this.manager).restart();
  }

  public async serverPath(): Promise<string> {
    return (await this.manager).serverPath();
  }

  public async stop(): Promise<void> {
    await (await this.manager).stop();
  }
}
