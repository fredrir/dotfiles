import * as vscode from "vscode";
import { SECTION } from "./languages";

export type TscScope = "open" | "project";
export type FixesOnSave = "off" | "safe" | "unsafe";

export interface Settings {
  typeAware: boolean;
  syntaxDiagnostics: boolean;
  tscScope: TscScope;
  debounceMs: number;
  tsdk: string;
  fixesOnSave: FixesOnSave;
}

export function readSettings(
  scope?: vscode.ConfigurationScope | null,
): Settings {
  const config = vscode.workspace.getConfiguration(SECTION, scope);
  return {
    typeAware: config.get("typeAware", true),
    syntaxDiagnostics: config.get("diagnostics.syntax", true),
    tscScope: config.get<TscScope>("diagnostics.tscScope", "open"),
    debounceMs: config.get("diagnostics.debounceMs", 250),
    tsdk: config.get("typescript.tsdk", "").trim(),
    fixesOnSave: config.get<FixesOnSave>("fixes.onSave", "off"),
  };
}
