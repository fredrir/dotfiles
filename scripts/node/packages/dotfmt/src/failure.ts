import os from "node:os";
import * as vscode from "vscode";
import type { Failed } from "./lib/process";

export interface Location {
  file: string;
  line: number;
  column?: number;
  message: string;
}

export interface Failure {
  message: string;
  log: string;
  locations: Location[];
}

const LOCATED = /^(.*dotfmt\.dotfile):(\d+):(?:(\d+):)? (.*)$/;

export function failure(failed: Failed, binary: string): Failure {
  if (failed.kind === "unavailable") {
    const code = "code" in failed.error ? failed.error.code : undefined;
    const message =
      code === "ENOENT"
        ? `dotfmt not found: ${tildify(binary)}`
        : code === "EACCES"
          ? `dotfmt not executable: ${tildify(binary)}`
          : `dotfmt failed to start: ${failed.error.message}`;
    return { message, log: message, locations: [] };
  }

  const lines = failed.stderr
    .split(/\r?\n/)
    .map((line) => line.replace(/^dotfmt: /, "").trim())
    .filter(Boolean);
  if (failed.signal || (failed.code !== 1 && failed.code !== 2)) {
    const message = `dotfmt crashed: ${failed.signal ?? `exit ${failed.code}`}`;
    return { message, log: [message, ...lines].join("\n"), locations: [] };
  }

  const locations = lines.flatMap((line): Location[] => {
    const [, file, row, column, message] = LOCATED.exec(line) ?? [];
    return file && row && message
      ? [
          {
            file,
            line: Number(row),
            column: column ? Number(column) : undefined,
            message,
          },
        ]
      : [];
  });
  const first = locations[0];
  const message = first
    ? `${display(first.file)}:${first.line}: ${first.message}`
    : (lines[0] ?? `dotfmt failed: exit ${failed.code}`);
  return { message, log: lines.join("\n") || message, locations };
}

export function range(location: Location): vscode.Range {
  const line = Math.max(location.line - 1, 0);
  return location.column === undefined
    ? new vscode.Range(line, 0, line, Number.MAX_SAFE_INTEGER)
    : new vscode.Range(line, location.column - 1, line, location.column);
}

export function tildify(file: string): string {
  const home = os.homedir();
  return file === home || file.startsWith(`${home}/`)
    ? `~${file.slice(home.length)}`
    : file;
}

function display(file: string): string {
  const relative = vscode.workspace.asRelativePath(file, false);
  return relative === file ? tildify(file) : relative;
}
