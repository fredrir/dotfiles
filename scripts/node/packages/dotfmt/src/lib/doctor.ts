import { isWithin } from "./paths";

export interface DoctorServer {
  name: string;
  version?: string;
}

export interface DoctorProject {
  root: string;
  tsconfig?: string;
  compiler?: string;
  version?: string;
  state?: string;
}

/** One resolved `dotfmt` settings section from the `dotfmt/doctor` response. */
export interface DoctorSettings {
  typeAware?: boolean;
  syntax?: boolean;
  tscScope?: string;
  debounceMs?: number;
  fixesOnSave?: string;
  tsdk?: string;
  hasFormat?: boolean;
  hasFormatExtra?: boolean;
}

/** A workspace folder's resolved settings. */
export interface DoctorFolder extends DoctorSettings {
  root: string;
}

/** The language server's `dotfmt/doctor` response. */
export interface DoctorReport {
  server: DoctorServer;
  trusted?: boolean;
  typeAware?: boolean;
  syntax?: boolean;
  tscScope?: string;
  default?: DoctorSettings;
  folders?: DoctorFolder[];
  roots: string[];
  projects: DoctorProject[];
}

export interface DoctorRequester {
  request<T>(method: string, params: unknown): Promise<T>;
}

export interface DoctorResult {
  report?: DoctorReport;
  error?: string;
}

/** Calls `dotfmt/doctor`, turning a missing request on older servers into a fallback value. */
export async function requestDoctor(
  client: DoctorRequester,
): Promise<DoctorResult> {
  try {
    const report = parseDoctorReport(
      await client.request<unknown>("dotfmt/doctor", {}),
    );
    return report ? { report } : { error: "unexpected response" };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

/** Reads the doctor response, tolerating fields an older server does not send. */
export function parseDoctorReport(value: unknown): DoctorReport | undefined {
  const report = object(value);
  const server = object(report?.server);
  if (!report || !server) {
    return undefined;
  }
  return {
    server: {
      name: text(server.name) ?? "dotfmt-lsp",
      version: text(server.version),
    },
    trusted: flag(report.trusted),
    typeAware: flag(report.typeAware),
    syntax: flag(report.syntax),
    tscScope: text(report.tscScope),
    default: settings(report.default),
    folders: objects(report.folders).flatMap((folder) => {
      const root = text(folder.root);
      return root ? [{ root, ...(settings(folder) ?? {}) }] : [];
    }),
    roots: strings(report.roots),
    projects: objects(report.projects).flatMap((project) => {
      const root = text(project.root);
      return root
        ? [
            {
              root,
              tsconfig: text(project.tsconfig),
              compiler: text(project.compiler),
              version: text(project.version),
              state: text(project.state),
            },
          ]
        : [];
    }),
  };
}

/** Human-readable doctor report lines; `shortenPath` is applied to every path. */
export function doctorReportLines(
  report: DoctorReport,
  shortenPath: (value: string) => string = (value) => value,
): string[] {
  const lines = [
    "server report:",
    `  server: ${report.server.name}${
      report.server.version
        ? ` ${report.server.version}`
        : " (version unavailable)"
    }`,
    `  trusted: ${yesNo(report.trusted)}`,
    `  type-aware: ${onOff(report.typeAware)}`,
    `  syntax diagnostics: ${onOff(report.syntax)}`,
    `  tsc scope: ${report.tscScope ?? "unknown"}`,
    ...(report.default
      ? [
          "  default settings:",
          ...settingsLines(report.default, shortenPath).map(
            (line) => `    ${line}`,
          ),
        ]
      : []),
    ...(report.folders && report.folders.length > 0
      ? [
          "  folders:",
          ...report.folders.flatMap((folder) => [
            `    ${shortenPath(folder.root)}`,
            ...settingsLines(folder, shortenPath).map(
              (line) => `      ${line}`,
            ),
          ]),
        ]
      : []),
    `  roots: ${
      report.roots.length > 0
        ? report.roots.map(shortenPath).join(", ")
        : "none"
    }`,
    "  projects:",
  ];
  if (report.projects.length === 0) {
    lines.push("    (none)");
    return lines;
  }
  for (const project of report.projects) {
    lines.push(
      `    ${shortenPath(project.root)}`,
      `      tsconfig: ${
        project.tsconfig ? shortenPath(project.tsconfig) : "none"
      }`,
      `      compiler: ${
        project.compiler ? shortenPath(project.compiler) : "unavailable"
      }`,
      `      version: ${project.version ?? "unknown"}`,
      `      state: ${project.state ?? "unknown"}`,
    );
  }
  return lines;
}

/** The server project whose root contains `file`, choosing the nearest root. */
export function projectForFile(
  report: DoctorReport,
  file: string,
): DoctorProject | undefined {
  return report.projects
    .filter((project) => isWithin(project.root, file))
    .sort((left, right) => right.root.length - left.root.length)[0];
}

/** The client-visible TypeScript state from workspace trust and resource settings. */
export function tscState(
  trusted: boolean,
  typeAware: boolean,
  scope: string,
): string {
  if (!trusted) {
    return "off (untrusted)";
  }
  return typeAware ? `on (${scope})` : "off";
}

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function settings(value: unknown): DoctorSettings | undefined {
  const found = object(value);
  if (!found) {
    return undefined;
  }
  return {
    typeAware: flag(found?.typeAware),
    syntax: flag(found?.syntax),
    tscScope: text(found?.tscScope),
    debounceMs: number(found?.debounceMs),
    fixesOnSave: text(found?.fixesOnSave),
    tsdk: text(found?.tsdk),
    hasFormat: flag(found?.hasFormat),
    hasFormatExtra: flag(found?.hasFormatExtra),
  };
}

function settingsLines(
  value: DoctorSettings,
  shortenPath: (path: string) => string,
): string[] {
  return [
    `type-aware: ${onOff(value.typeAware)}`,
    `syntax diagnostics: ${onOff(value.syntax)}`,
    `tsc scope: ${value.tscScope ?? "unknown"}`,
    `debounce: ${value.debounceMs === undefined ? "unknown" : `${value.debounceMs} ms`}`,
    `fixes on save: ${value.fixesOnSave ?? "unknown"}`,
    `tsdk: ${value.tsdk ? shortenPath(value.tsdk) : "auto"}`,
  ];
}

function objects(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value)
    ? value.flatMap((item) => {
        const found = object(item);
        return found ? [found] : [];
      })
    : [];
}

function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.flatMap((item) => (typeof item === "string" ? [item] : []))
    : [];
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function flag(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

function number(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function yesNo(value: boolean | undefined): string {
  return value === undefined ? "unknown" : value ? "yes" : "no";
}

function onOff(value: boolean | undefined): string {
  return value === undefined ? "unknown" : value ? "on" : "off";
}
