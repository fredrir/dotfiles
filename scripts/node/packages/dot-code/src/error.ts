import * as vscode from "vscode";
import {
  CloseAction,
  type CloseHandlerResult,
  ErrorAction,
  type ErrorHandler,
  type ErrorHandlerResult,
  type Message,
} from "vscode-languageclient/node";
import type { CrashTracker } from "./crash";

export class DotCodeErrorHandler implements ErrorHandler {
  constructor(
    private readonly crashTracker: CrashTracker,
    private readonly outputChannel: vscode.LogOutputChannel,
    private readonly onRestart: () => Promise<void>,
    private readonly isManualShutdown: () => boolean,
  ) {}

  public error(
    error: Error,
    message: Message | undefined,
    count: number | undefined,
  ): ErrorHandlerResult {
    const detail = message ? ` ${JSON.stringify(message)}` : "";
    this.outputChannel.error(
      `server error ${count ?? 1}: ${error.message}${detail}`,
    );

    if (count !== undefined && count <= 3) {
      return { action: ErrorAction.Continue, handled: true };
    }
    return { action: ErrorAction.Shutdown, handled: true };
  }

  public async closed(): Promise<CloseHandlerResult> {
    if (this.isManualShutdown()) {
      return { action: CloseAction.DoNotRestart, handled: true };
    }

    const { isCrashLoop, crashCount } = this.crashTracker.recordCrash();

    if (isCrashLoop) {
      this.outputChannel.error(
        `server crashed ${crashCount} times, not restarting`,
      );

      void vscode.window
        .showErrorMessage(
          "Dot Code server keeps crashing",
          "Restart",
          "Show Log",
        )
        .then((selected) => {
          if (selected === "Restart") {
            void this.onRestart();
          } else if (selected === "Show Log") {
            this.outputChannel.show(true);
          }
        });

      return { action: CloseAction.DoNotRestart, handled: true };
    }

    const backoffMs = Math.min(500 * Math.pow(2, crashCount - 1), 8000);
    this.outputChannel.warn(
      `server closed (${crashCount}), restarting in ${backoffMs}ms`,
    );

    await new Promise((resolve) => setTimeout(resolve, backoffMs));

    return { action: CloseAction.Restart, handled: true };
  }
}
