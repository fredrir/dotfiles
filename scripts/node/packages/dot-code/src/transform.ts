import * as vscode from "vscode";
import { TRANSFORM_CASE } from "./lib/constants";
import type { Server } from "./server";

export type Case =
  "camel" | "kebab" | "lower" | "pascal" | "snake" | "title" | "upper";

export type Target = "editor" | "input" | "fileName";

type Scope =
  | { scope: "selection"; text: string }
  | { scope: "cursor"; before: string; after: string }
  | { scope: "fileName"; name: string };

interface Replacement {
  text: string;
  before: number;
  after: number;
}

interface Edit {
  start: number;
  end: number;
  range: vscode.Range;
  text: string;
}

const CURSOR = "\uE000";

let transforming = false;

function convert(
  server: Server,
  kind: Case,
  targets: Scope[],
): Promise<Replacement[]> {
  return server.request<Replacement[]>(TRANSFORM_CASE, { case: kind, targets });
}

async function convertOne(
  server: Server,
  kind: Case,
  target: Scope,
): Promise<Replacement> {
  const [replacement] = await convert(server, kind, [target]);
  if (!replacement) {
    throw new Error("transform returned nothing");
  }
  return replacement;
}

function scopeOf(
  document: vscode.TextDocument,
  selection: vscode.Selection,
): Scope {
  if (!selection.isEmpty) {
    return { scope: "selection", text: document.getText(selection) };
  }
  const line = document.lineAt(selection.active).text;
  const column = selection.active.character;
  return {
    scope: "cursor",
    before: line.slice(0, column),
    after: line.slice(column),
  };
}

function nonOverlapping(edits: Edit[]): Edit[] {
  const kept: Edit[] = [];
  for (const edit of edits.toSorted((a, b) => a.start - b.start)) {
    if (edit.start >= (kept.at(-1)?.end ?? 0)) {
      kept.push(edit);
    }
  }
  return kept;
}

function shift(offset: number, edits: Edit[]): number {
  let delta = 0;
  for (const { start, end, text } of edits) {
    if (end <= offset) {
      delta += text.length - (end - start);
    } else if (start < offset) {
      return start + delta + Math.min(offset - start, text.length);
    } else {
      break;
    }
  }
  return offset + delta;
}

async function transformEditor(server: Server, kind: Case): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return;
  }
  const { document, selections } = editor;
  const version = document.version;
  const replacements = await convert(
    server,
    kind,
    selections.map((selection) => scopeOf(document, selection)),
  );
  if (document.version !== version) {
    return;
  }

  const edits = nonOverlapping(
    selections.flatMap((selection, index) => {
      const replacement = replacements[index];
      if (!replacement) {
        return [];
      }
      const start = document.offsetAt(selection.start) - replacement.before;
      const end = document.offsetAt(selection.end) + replacement.after;
      const range = new vscode.Range(
        document.positionAt(start),
        document.positionAt(end),
      );
      return document.getText(range) === replacement.text
        ? []
        : [{ start, end, range, text: replacement.text }];
    }),
  );
  if (edits.length === 0) {
    return;
  }

  const offsets = selections.map((selection) => ({
    anchor: document.offsetAt(selection.anchor),
    active: document.offsetAt(selection.active),
  }));
  const applied = await editor.edit((builder) => {
    for (const edit of edits) {
      builder.replace(edit.range, edit.text);
    }
  });
  if (applied) {
    editor.selections = offsets.map(
      ({ anchor, active }) =>
        new vscode.Selection(
          document.positionAt(shift(anchor, edits)),
          document.positionAt(shift(active, edits)),
        ),
    );
  }
}

async function copy(): Promise<string> {
  await vscode.env.clipboard.writeText("");
  await vscode.commands.executeCommand("editor.action.clipboardCopyAction");
  return vscode.env.clipboard.readText();
}

async function paste(text: string): Promise<void> {
  await vscode.env.clipboard.writeText(text);
  // an empty paste may be ignored, so cut to clear the selection
  await vscode.commands.executeCommand(
    text
      ? "editor.action.clipboardPasteAction"
      : "editor.action.clipboardCutAction",
  );
}

async function selectAll(): Promise<void> {
  await vscode.commands.executeCommand("editor.action.selectAll");
}

async function transformInputCursor(server: Server, kind: Case): Promise<void> {
  await paste(CURSOR);
  await selectAll();
  const text = await copy();
  const cursor = text.lastIndexOf(CURSOR);
  if (cursor === -1) {
    return;
  }
  const before = text.slice(0, cursor);
  const after = text.slice(cursor + CURSOR.length);
  let replacement = before + after;
  try {
    const converted = await convertOne(server, kind, {
      scope: "cursor",
      before,
      after,
    });
    replacement =
      before.slice(0, before.length - converted.before) +
      converted.text +
      after.slice(converted.after);
  } finally {
    await paste(replacement);
  }
}

async function transformInput(
  server: Server,
  kind: Case,
  fileName: boolean,
): Promise<void> {
  const selection = await copy();
  if (fileName) {
    if (!selection) {
      await selectAll();
    }
    const name = selection || (await copy());
    if (name) {
      const converted = await convertOne(server, kind, {
        scope: "fileName",
        name,
      });
      await paste(converted.text);
    }
  } else if (selection) {
    const converted = await convertOne(server, kind, {
      scope: "selection",
      text: selection,
    });
    await paste(converted.text);
  } else {
    await transformInputCursor(server, kind);
  }
}

export async function transformCase(
  server: Server,
  kind: Case,
  target: Target,
): Promise<void> {
  if (target === "editor") {
    await transformEditor(server, kind);
    return;
  }
  if (transforming) {
    return;
  }
  transforming = true;
  const clipboard = await vscode.env.clipboard.readText();
  try {
    await transformInput(server, kind, target === "fileName");
  } finally {
    await vscode.env.clipboard.writeText(clipboard);
    transforming = false;
  }
}
