import type * as vscode from "vscode";
import type { Client } from "../client";

export interface Explanation {
  language?: string;
  dialect?: string;
  skipped?: string;
  configs: string[];
  error?: string;
}

/** The server's view of the language, dialect and configuration chain for a buffer. */
export function explain(
  client: Client,
  uri: vscode.Uri,
  languageId: string,
): Promise<Explanation> {
  return client.request<Explanation>("dotfmt/explain", {
    textDocument: { uri: uri.toString() },
    languageId,
  });
}
