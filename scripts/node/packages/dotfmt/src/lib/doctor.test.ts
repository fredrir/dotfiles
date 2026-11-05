import { describe, expect, test } from "bun:test";
import {
  doctorReportLines,
  parseDoctorReport,
  projectForFile,
  requestDoctor,
  tscState,
  type DoctorReport,
} from "./doctor";

const response = {
  server: { name: "dotfmt-lsp", version: "0.3.0" },
  trusted: true,
  typeAware: true,
  syntax: false,
  tscScope: "project",
  roots: ["/work/app", "/work/other"],
  projects: [
    {
      root: "/work/app",
      tsconfig: "/work/app/tsconfig.json",
      compiler: "/work/app/node_modules/.bin/tsc",
      version: "5.9.3",
      state: "ok",
    },
    {
      root: "/work/other",
      tsconfig: null,
      compiler: null,
      version: null,
      state: "no-tsconfig",
    },
  ],
};

describe("parseDoctorReport", () => {
  test("reads the server response and tolerates null fields", () => {
    const report = parseDoctorReport(response);
    expect(report).toEqual({
      server: { name: "dotfmt-lsp", version: "0.3.0" },
      trusted: true,
      typeAware: true,
      syntax: false,
      tscScope: "project",
      default: undefined,
      folders: [],
      roots: ["/work/app", "/work/other"],
      projects: [
        {
          root: "/work/app",
          tsconfig: "/work/app/tsconfig.json",
          compiler: "/work/app/node_modules/.bin/tsc",
          version: "5.9.3",
          state: "ok",
        },
        {
          root: "/work/other",
          tsconfig: undefined,
          compiler: undefined,
          version: undefined,
          state: "no-tsconfig",
        },
      ],
    });
  });

  test("accepts a partial response from an older server", () => {
    expect(parseDoctorReport({ server: { name: "dotfmt-lsp" } })).toEqual({
      server: { name: "dotfmt-lsp", version: undefined },
      trusted: undefined,
      typeAware: undefined,
      syntax: undefined,
      tscScope: undefined,
      default: undefined,
      folders: [],
      roots: [],
      projects: [],
    });
  });

  test("reads the default and per-folder settings, tolerating null fields", () => {
    const report = parseDoctorReport({
      server: { name: "dotfmt-lsp" },
      default: {
        typeAware: true,
        syntax: false,
        tscScope: "open",
        debounceMs: 250,
        fixesOnSave: "off",
        tsdk: null,
        hasFormat: false,
      },
      folders: [
        {
          root: "/work/app",
          typeAware: true,
          syntax: false,
          tscScope: "project",
          debounceMs: 500,
          fixesOnSave: "safe",
          tsdk: "/sdk/typescript",
          hasFormat: true,
        },
        {
          root: "/work/other",
          typeAware: null,
          syntax: null,
          tscScope: null,
          debounceMs: null,
          fixesOnSave: null,
          tsdk: null,
        },
      ],
    });
    expect(report?.default).toEqual({
      typeAware: true,
      syntax: false,
      tscScope: "open",
      debounceMs: 250,
      fixesOnSave: "off",
      tsdk: undefined,
      hasFormat: false,
      hasFormatExtra: undefined,
    });
    expect(report?.folders).toEqual([
      {
        root: "/work/app",
        typeAware: true,
        syntax: false,
        tscScope: "project",
        debounceMs: 500,
        fixesOnSave: "safe",
        tsdk: "/sdk/typescript",
        hasFormat: true,
        hasFormatExtra: undefined,
      },
      {
        root: "/work/other",
        typeAware: undefined,
        syntax: undefined,
        tscScope: undefined,
        debounceMs: undefined,
        fixesOnSave: undefined,
        tsdk: undefined,
        hasFormat: undefined,
        hasFormatExtra: undefined,
      },
    ]);
  });

  test("rejects a response without a server object", () => {
    expect(parseDoctorReport(null)).toBeUndefined();
    expect(parseDoctorReport({})).toBeUndefined();
  });
});

describe("doctorReportLines", () => {
  test("formats settings, roots and every project", () => {
    const report = parseDoctorReport(response) as DoctorReport;
    expect(doctorReportLines(report)).toEqual([
      "server report:",
      "  server: dotfmt-lsp 0.3.0",
      "  trusted: yes",
      "  type-aware: on",
      "  syntax diagnostics: off",
      "  tsc scope: project",
      "  roots: /work/app, /work/other",
      "  projects:",
      "    /work/app",
      "      tsconfig: /work/app/tsconfig.json",
      "      compiler: /work/app/node_modules/.bin/tsc",
      "      version: 5.9.3",
      "      state: ok",
      "    /work/other",
      "      tsconfig: none",
      "      compiler: unavailable",
      "      version: unknown",
      "      state: no-tsconfig",
    ]);
  });

  test("renders the default and per-folder settings", () => {
    const report = parseDoctorReport({
      server: { name: "dotfmt-lsp", version: "0.3.0" },
      default: {
        typeAware: true,
        syntax: false,
        tscScope: "open",
        debounceMs: 250,
        fixesOnSave: "off",
        tsdk: null,
        hasFormat: true,
        hasFormatExtra: false,
      },
      folders: [
        {
          root: "/work/app",
          typeAware: true,
          syntax: false,
          tscScope: "project",
          debounceMs: 500,
          fixesOnSave: "safe",
          tsdk: "/sdk/typescript",
          hasFormat: false,
          hasFormatExtra: true,
        },
      ],
      roots: ["/work/app"],
    }) as DoctorReport;
    expect(
      doctorReportLines(report, (value) => value.replace("/work", "~")),
    ).toEqual([
      "server report:",
      "  server: dotfmt-lsp 0.3.0",
      "  trusted: unknown",
      "  type-aware: unknown",
      "  syntax diagnostics: unknown",
      "  tsc scope: unknown",
      "  default settings:",
      "    type-aware: on",
      "    syntax diagnostics: off",
      "    tsc scope: open",
      "    debounce: 250 ms",
      "    fixes on save: off",
      "    tsdk: auto",
      "    native format: on",
      "    format extra: off",
      "  folders:",
      "    ~/app",
      "      type-aware: on",
      "      syntax diagnostics: off",
      "      tsc scope: project",
      "      debounce: 500 ms",
      "      fixes on save: safe",
      "      tsdk: /sdk/typescript",
      "      native format: off",
      "      format extra: on",
      "  roots: ~/app",
      "  projects:",
      "    (none)",
    ]);
  });

  test("shortens paths and marks an empty project list", () => {
    const report = parseDoctorReport({
      server: { name: "dotfmt-lsp", version: "0.3.0" },
      roots: ["/work/app"],
    }) as DoctorReport;
    expect(
      doctorReportLines(report, (value) => value.replace("/work", "~")),
    ).toEqual([
      "server report:",
      "  server: dotfmt-lsp 0.3.0",
      "  trusted: unknown",
      "  type-aware: unknown",
      "  syntax diagnostics: unknown",
      "  tsc scope: unknown",
      "  roots: ~/app",
      "  projects:",
      "    (none)",
    ]);
  });
});

describe("projectForFile", () => {
  test("chooses the nearest workspace root", () => {
    const report = parseDoctorReport({
      server: { name: "dotfmt-lsp" },
      projects: [
        { root: "/work", state: "ok" },
        { root: "/work/app", state: "backoff" },
      ],
    }) as DoctorReport;
    expect(projectForFile(report, "/work/app/src/main.ts")?.state).toBe(
      "backoff",
    );
    expect(projectForFile(report, "/elsewhere/main.ts")).toBeUndefined();
  });
});

describe("tscState", () => {
  test("reports trust, type-aware and scope", () => {
    expect(tscState(false, true, "open")).toBe("off (untrusted)");
    expect(tscState(true, false, "open")).toBe("off");
    expect(tscState(true, true, "project")).toBe("on (project)");
  });
});

describe("requestDoctor", () => {
  test("calls the doctor request with empty params", async () => {
    const client = {
      request<T>(method: string, params: unknown): Promise<T> {
        expect(method).toBe("dotfmt/doctor");
        expect(params).toEqual({});
        return Promise.resolve(response as T);
      },
    };
    expect((await requestDoctor(client)).report?.server.version).toBe("0.3.0");
  });

  test("falls back when an older server does not implement the request", async () => {
    const client = {
      request<T>(): Promise<T> {
        return Promise.reject(new Error("Method not found"));
      },
    };
    expect(await requestDoctor(client)).toEqual({
      error: "Method not found",
    });
  });
});
