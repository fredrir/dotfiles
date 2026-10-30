import path from "node:path";
import picomatch from "picomatch";
import * as vscode from "vscode";

type Compare = (a: string, b: string) => number;
type Lexicographic = "default" | "upper" | "lower" | "unicode";
type Exclude = Record<string, boolean | { when?: unknown }>;

interface Entry {
  name: string;
  mtime: number;
}

const numeric = new Intl.Collator(undefined, { numeric: true });
const accent = new Intl.Collator(undefined, {
  numeric: true,
  sensitivity: "accent",
});

const collate = (collator: Intl.Collator, a: string, b: string) =>
  collator.compare(a, b) || a.length - b.length;

const unicode: Compare = (a, b) => (a === b ? 0 : a < b ? -1 : 1);

const isUpper = (name: string) =>
  name.charAt(0).toLocaleLowerCase() !== name.charAt(0);
const isLower = (name: string) =>
  name.charAt(0).toLocaleUpperCase() !== name.charAt(0);
const upperFirst: Compare = (a, b) =>
  isUpper(a) && isLower(b) ? -1 : isLower(a) && isUpper(b) ? 1 : 0;

const BY_NAME: Record<Lexicographic, Compare> = {
  default: (a, b) => collate(numeric, a, b),
  upper: (a, b) => upperFirst(a, b) || collate(numeric, a, b),
  lower: (a, b) => upperFirst(b, a) || collate(numeric, a, b),
  unicode,
};

function extension(name: string): string {
  const [, stem, ext] = /^(.*?)(?:\.([^.]*))?$/.exec(name) ?? [];
  return (stem && !stem.startsWith(".") && ext) || "";
}

function byExtension(lexicographic: Lexicographic): Compare {
  const byName = BY_NAME[lexicographic];
  if (lexicographic === "unicode") {
    return (a, b) =>
      unicode(extension(a).toLowerCase(), extension(b).toLowerCase()) ||
      byName(a, b);
  }
  return (a, b) => collate(accent, extension(a), extension(b)) || byName(a, b);
}

function comparator(
  config: vscode.WorkspaceConfiguration,
): (a: Entry, b: Entry) => number {
  const option = config.get<string>("sortOrderLexicographicOptions", "");
  const lexicographic = Object.hasOwn(BY_NAME, option)
    ? (option as Lexicographic)
    : "default";
  const byName = BY_NAME[lexicographic];
  const compare = (() => {
    switch (config.get<string>("sortOrder")) {
      case "type": {
        const byType = byExtension(lexicographic);
        return (a: Entry, b: Entry) => byType(a.name, b.name);
      }
      case "modified":
        return (a: Entry, b: Entry) =>
          b.mtime - a.mtime || byName(a.name, b.name);
      default:
        return (a: Entry, b: Entry) => byName(a.name, b.name);
    }
  })();
  return config.get<boolean>("sortOrderReverse")
    ? (a, b) => compare(b, a)
    : compare;
}

function exclusion(
  dir: vscode.Uri,
  names: string[],
): (name: string) => boolean {
  const exclude = vscode.workspace
    .getConfiguration("files", dir)
    .get<Exclude>("exclude", {});
  const root = vscode.workspace.getWorkspaceFolder(dir)?.uri.path ?? dir.path;
  const prefix = path.posix.relative(root, dir.path);
  const siblings = new Set(names);
  const rules = Object.entries(exclude)
    .filter(([, value]) => value)
    .map(([pattern, value]) => ({
      matches: picomatch(pattern, { dot: true, nonegate: true }),
      when:
        typeof value === "object" && typeof value.when === "string"
          ? value.when
          : undefined,
    }));

  return (name) => {
    const stem = name.slice(0, name.length - path.extname(name).length);
    return rules.some(
      ({ matches, when }) =>
        matches(path.posix.join(prefix, name)) &&
        (when === undefined ||
          siblings.has(when.replace("$(basename)", () => stem))),
    );
  };
}

function activeFile(): vscode.Uri | undefined {
  const input = vscode.window.tabGroups.activeTabGroup.activeTab?.input;
  if (
    input instanceof vscode.TabInputText ||
    input instanceof vscode.TabInputCustom ||
    input instanceof vscode.TabInputNotebook
  ) {
    return input.uri;
  }
  return undefined;
}

export async function navigate(offset: 1 | -1): Promise<void> {
  const current = activeFile();
  if (current?.scheme !== "file") {
    return;
  }

  const dir = vscode.Uri.joinPath(current, "..");
  const name = path.posix.basename(current.path);
  const config = vscode.workspace.getConfiguration("explorer", dir);
  const modified = config.get<string>("sortOrder") === "modified";

  const children = await vscode.workspace.fs.readDirectory(dir);
  const isExcluded = exclusion(
    dir,
    children.map(([child]) => child),
  );
  const files = await Promise.all(
    children
      .filter(
        ([child, type]) =>
          type & vscode.FileType.File && (child === name || !isExcluded(child)),
      )
      .map(async ([child]) => ({
        name: child,
        mtime: modified
          ? (await vscode.workspace.fs.stat(vscode.Uri.joinPath(dir, child)))
              .mtime
          : 0,
      })),
  );
  files.sort(comparator(config));

  const index = files.findIndex((file) => file.name === name);
  const target = files[(index + offset + files.length) % files.length];
  if (index === -1 || !target || target.name === name) {
    return;
  }
  await vscode.commands.executeCommand(
    "vscode.open",
    vscode.Uri.joinPath(dir, target.name),
  );
}
