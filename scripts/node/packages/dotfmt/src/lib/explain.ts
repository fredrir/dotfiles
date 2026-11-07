import type * as vscode from "vscode";
import type { Client } from "../client";

export interface Explanation {
  language?: string;
  dialect?: string;
  skipped?: string;
  configs: string[];
  error?: string;
}

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
