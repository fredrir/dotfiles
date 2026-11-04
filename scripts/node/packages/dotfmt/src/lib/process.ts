import { spawn } from "node:child_process";
import type * as vscode from "vscode";

export type Run =
  | { kind: "ok"; stdout: string }
  | {
      kind: "failed";
      code: number | null;
      signal: NodeJS.Signals | null;
      stderr: string;
    }
  | { kind: "unavailable"; error: Error }
  | { kind: "cancelled" };

export type Failed = Extract<Run, { kind: "failed" | "unavailable" }>;

export interface Invocation {
  binary: string;
  args: string[];
  cwd: string;
  input?: string;
  token?: vscode.CancellationToken;
}

export function run({
  binary,
  args,
  cwd,
  input,
  token,
}: Invocation): Promise<Run> {
  if (token?.isCancellationRequested) {
    return Promise.resolve({ kind: "cancelled" });
  }
  return new Promise((resolve) => {
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let cancellation: vscode.Disposable | undefined;
    let settled = false;
    const settle = (result: Run): void => {
      if (!settled) {
        settled = true;
        cancellation?.dispose();
        resolve(result);
      }
    };

    const child = spawn(binary, args, {
      cwd,
      stdio: "pipe",
      windowsHide: true,
    });
    cancellation = token?.onCancellationRequested(() => {
      child.kill();
      settle({ kind: "cancelled" });
    });
    child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
    child.on("error", (error) => settle({ kind: "unavailable", error }));
    child.on("close", (code, signal) =>
      settle(
        code === 0
          ? { kind: "ok", stdout: Buffer.concat(stdout).toString("utf8") }
          : {
              kind: "failed",
              code,
              signal,
              stderr: Buffer.concat(stderr).toString("utf8"),
            },
      ),
    );
    child.stdin.on("error", () => undefined);
    child.stdin.end(input ?? "", "utf8");
  });
}
