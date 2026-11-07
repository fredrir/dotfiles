export type TraceLevel = "off" | "messages" | "verbose";

export function traceLevel(value: unknown): TraceLevel {
  return value === "messages" || value === "verbose" ? value : "off";
}
