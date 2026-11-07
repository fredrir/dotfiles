import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const PACKAGE_ROOT = import.meta.dir;
const SCHEMA_PATH = path.join(PACKAGE_ROOT, "schema.json");
const PACKAGE_PATH = path.join(PACKAGE_ROOT, "package.json");

const DOTFMT_ROOT = path.resolve(PACKAGE_ROOT, "../../../rust/crates/dotfmt");
const CORE_PARSE_PATH = path.join(DOTFMT_ROOT, "core/src/config/parse.rs");
const LANGUAGES_PATH = path.join(DOTFMT_ROOT, "languages");

const SETTING_PREFIX = "dotfmt.format.";

const LANGUAGES = [
  "bib",
  "conf",
  "css",
  "env",
  "html",
  "ignore",
  "ini",
  "json",
  "javascript",
  "latex",
  "lua",
  "markdown",
  "plist",
  "toml",
  "typescript",
  "yaml",
] as const;

const SHARED_CONFIGS: Record<string, readonly string[]> = {
  typescript: ["typescript", "javascript"],
};

type OptionType = "boolean" | "integer" | "string" | "array";

interface Option {
  type: OptionType;
  enum?: (string | boolean)[];
  minimum?: number;
  maximum?: number;
  items?: { type: "string" };
  default?: string | number | boolean;
  defaultDescription?: string;
  description: string;
}

interface Schema {
  global: Record<string, Option>;
  files: Record<string, Option>;
  common: Record<string, Option>;
  languages: Record<string, Record<string, Option>>;
}

interface Manifest {
  contributes?: {
    configuration?: { properties?: Record<string, unknown> };
  };
}

const OPTION_TYPES: readonly OptionType[] = [
  "boolean",
  "integer",
  "string",
  "array",
];

function fail(message: string): never {
  throw new Error(message);
}

function readSchema(): Schema {
  const schema = JSON.parse(readFileSync(SCHEMA_PATH, "utf8")) as Schema;
  for (const scope of ["global", "files", "common"] as const) {
    validateOptions(scope, schema[scope]);
  }
  for (const [language, options] of Object.entries(schema.languages)) {
    validateOptions(language, options);
  }
  return schema;
}

function validateOptions(scope: string, options: Record<string, Option>): void {
  for (const [key, option] of Object.entries(options)) {
    const where = `${scope}.${key}`;
    if (!OPTION_TYPES.includes(option.type)) {
      fail(`schema.json: ${where} has type ${JSON.stringify(option.type)}`);
    }
    if (!option.description) {
      fail(`schema.json: ${where} has no description`);
    }
    if (option.default === undefined && !option.defaultDescription) {
      fail(`schema.json: ${where} documents no default`);
    }
    if (
      option.default !== undefined &&
      option.defaultDescription !== undefined
    ) {
      fail(`schema.json: ${where} has both a default and a defaultDescription`);
    }
    if (option.enum) {
      if (option.type !== "string" && option.type !== "boolean") {
        fail(
          `schema.json: ${where} has enum values but is not a string or boolean`,
        );
      }
      if (
        option.default !== undefined &&
        !option.enum.some((value) => String(value) === String(option.default))
      ) {
        fail(`schema.json: ${where} default is not one of its enum values`);
      }
    }
    if (option.minimum !== undefined || option.maximum !== undefined) {
      if (option.type !== "integer") {
        fail(`schema.json: ${where} has a range but is not an integer`);
      }
      if (
        option.minimum !== undefined &&
        option.maximum !== undefined &&
        option.minimum > option.maximum
      ) {
        fail(`schema.json: ${where} has an empty range`);
      }
    }
    if (option.type === "integer" && typeof option.default === "number") {
      const { minimum, maximum } = option;
      if (
        (minimum !== undefined && option.default < minimum) ||
        (maximum !== undefined && option.default > maximum)
      ) {
        fail(`schema.json: ${where} default is outside its range`);
      }
    }
    if (option.type === "array" && option.items?.type !== "string") {
      fail(`schema.json: ${where} is an array without string items`);
    }
  }
}

function settingsProperty(option: Option): Record<string, unknown> {
  const property: Record<string, unknown> = { type: option.type, default: null };
  if (option.items) {
    property.items = option.items;
  }
  if (option.enum) {
    property.enum = option.enum;
  }
  if (option.minimum !== undefined) {
    property.minimum = option.minimum;
  }
  if (option.maximum !== undefined) {
    property.maximum = option.maximum;
  }
  property.scope = "resource";
  property.markdownDescription = markdownDescription(option);
  return property;
}

function markdownDescription(option: Option): string {
  const stated =
    option.default !== undefined
      ? `\`${String(option.default)}\``
      : option.defaultDescription;
  return [
    option.description,
    `Default: ${stated}. Unset here inherits from \`dotfmt.dotfile\`.`,
  ].join("\n\n");
}

function generatedSettings(schema: Schema): Record<string, unknown> {
  const generated: Record<string, unknown> = {};
  for (const [key, option] of Object.entries(schema.global)) {
    generated[`${SETTING_PREFIX}global.${key}`] = settingsProperty(option);
  }
  for (const [key, option] of Object.entries(schema.files)) {
    generated[`${SETTING_PREFIX}${key}`] = settingsProperty(option);
  }
  for (const [language, options] of Object.entries(schema.languages)) {
    for (const [key, option] of Object.entries(schema.common)) {
      generated[`${SETTING_PREFIX}${language}.${key}`] =
        settingsProperty(option);
    }
    for (const [key, option] of Object.entries(options)) {
      generated[`${SETTING_PREFIX}${language}.${key}`] =
        settingsProperty(option);
    }
  }
  return generated;
}

function readManifest(): { manifest: Manifest; text: string } {
  const text = readFileSync(PACKAGE_PATH, "utf8");
  return { manifest: JSON.parse(text) as Manifest, text };
}

function propertiesOf(manifest: Manifest): Record<string, unknown> {
  const properties = manifest.contributes?.configuration?.properties;
  if (!properties) {
    fail("package.json has no contributes.configuration.properties");
  }
  return properties;
}

function withGeneratedSettings(
  manifest: Manifest,
  generated: Record<string, unknown>,
): string {
  const properties = propertiesOf(manifest);
  const kept = Object.fromEntries(
    Object.entries(properties).filter(
      ([key]) => !key.startsWith(SETTING_PREFIX),
    ),
  );
  const configuration = manifest.contributes?.configuration;
  if (!configuration) {
    fail("package.json has no contributes.configuration");
  }
  configuration.properties = { ...kept, ...generated };
  return `${JSON.stringify(manifest, null, 2)}\n`;
}

function staleSummary(
  properties: Record<string, unknown>,
  generated: Record<string, unknown>,
): string[] {
  const current = Object.keys(properties).filter((key) =>
    key.startsWith(SETTING_PREFIX),
  );
  const missing = Object.keys(generated).filter((key) => !(key in properties));
  const leftover = current.filter((key) => !(key in generated));
  const changed = current.filter(
    (key) =>
      key in generated &&
      JSON.stringify(properties[key]) !== JSON.stringify(generated[key]),
  );
  const summary = [
    `${missing.length} missing`,
    `${leftover.length} leftover`,
    `${changed.length} changed`,
  ];
  const lines = [`package.json is stale: ${summary.join(", ")}.`];
  const named = [...missing, ...leftover, ...changed].slice(0, 10);
  if (named.length > 0) {
    lines.push(`  ${named.join("\n  ")}`);
  }
  return lines;
}

function main(): void {
  const args = process.argv.slice(2);
  for (const arg of args) {
    if (arg !== "--check") {
      fail(`unknown argument: ${arg}`);
    }
  }
  const check = args.includes("--check");

  const schema = readSchema();
  if (!checkCoverage(schema)) {
    process.exitCode = 1;
    return;
  }
  const generated = generatedSettings(schema);

  const { manifest, text } = readManifest();
  const current = propertiesOf(manifest);
  const next = withGeneratedSettings(manifest, generated);
  if (next === text) {
    console.log(
      `${SETTING_PREFIX}*: ${Object.keys(generated).length} settings up to date`,
    );
    return;
  }
  if (!check) {
    writeFileSync(PACKAGE_PATH, next);
    console.log(
      `${SETTING_PREFIX}*: wrote ${Object.keys(generated).length} settings`,
    );
    return;
  }
  console.error(staleSummary(current, generated).join("\n"));
  console.error("run: bun generate-settings.ts");
  process.exitCode = 1;
}

interface StringLiteral {
  value: string;
  end: number;
}

function readString(source: string, start: number): StringLiteral {
  let value = "";
  let index = start + 1;
  while (index < source.length) {
    const character = source[index]!;
    if (character === "\\") {
      value += source[index + 1] ?? "";
      index += 2;
      continue;
    }
    if (character === '"') {
      return { value, end: index + 1 };
    }
    value += character;
    index += 1;
  }
  return fail(`coverage: unterminated string at ${start}`);
}

function skipComment(source: string, index: number): number {
  if (source.startsWith("//", index)) {
    const newline = source.indexOf("\n", index);
    return newline < 0 ? source.length : newline + 1;
  }
  if (source.startsWith("/*", index)) {
    const close = source.indexOf("*/", index + 2);
    return close < 0 ? source.length : close + 2;
  }
  return -1;
}

function skipCharacterLiteral(source: string, index: number): number {
  const literal = /^'(?:\\?.)'/.exec(source.slice(index, index + 4));
  return literal ? index + literal[0].length : -1;
}

function skipBlock(source: string, start: number): number {
  let depth = 0;
  let index = start;
  while (index < source.length) {
    const character = source[index]!;
    if (character === '"') {
      index = readString(source, index).end;
      continue;
    }
    const comment = skipComment(source, index);
    if (comment >= 0) {
      index = comment;
      continue;
    }
    if (character === "{") {
      depth += 1;
    } else if (character === "}") {
      depth -= 1;
      if (depth === 0) {
        return index + 1;
      }
    }
    index += 1;
  }
  return fail(`coverage: unbalanced block at ${start}`);
}

function skipExpression(source: string, start: number): number {
  let braces = 0;
  let parens = 0;
  let brackets = 0;
  let index = start;
  while (index < source.length) {
    const character = source[index]!;
    if (character === '"') {
      index = readString(source, index).end;
      continue;
    }
    const comment = skipComment(source, index);
    if (comment >= 0) {
      index = comment;
      continue;
    }
    if (character === "'") {
      const literal = skipCharacterLiteral(source, index);
      if (literal >= 0) {
        index = literal;
        continue;
      }
    }
    if (character === "{") {
      braces += 1;
    } else if (character === "}") {
      if (braces === 0 && parens === 0 && brackets === 0) {
        return index;
      }
      braces -= 1;
    } else if (character === "(") {
      parens += 1;
    } else if (character === ")") {
      parens -= 1;
    } else if (character === "[") {
      brackets += 1;
    } else if (character === "]") {
      brackets -= 1;
    } else if (
      character === "," &&
      braces === 0 &&
      parens === 0 &&
      brackets === 0
    ) {
      return index;
    }
    index += 1;
  }
  return index;
}

function matchArms(source: string, marker: string): string[] {
  const markerAt = source.indexOf(marker);
  if (markerAt < 0) {
    fail(`coverage: cannot find ${JSON.stringify(marker)}`);
  }
  const matchAt = source.indexOf("match ", markerAt);
  if (matchAt < 0) {
    fail(`coverage: no match block after ${JSON.stringify(marker)}`);
  }
  const open = source.indexOf("{", matchAt);
  if (open < 0) {
    fail(`coverage: no match body after ${JSON.stringify(marker)}`);
  }

  const keys: string[] = [];
  let index = open + 1;
  const skip = (): void => {
    for (;;) {
      while (index < source.length && /\s/.test(source[index]!)) {
        index += 1;
      }
      const comment = skipComment(source, index);
      if (comment < 0) {
        return;
      }
      index = comment;
    }
  };

  for (;;) {
    skip();
    if (index >= source.length || source[index] === "}") {
      return keys;
    }
    if (source[index] === ",") {
      index += 1;
      continue;
    }
    const patterns: string[] = [];
    for (;;) {
      skip();
      if (source[index] !== '"') {
        break;
      }
      const literal = readString(source, index);
      patterns.push(literal.value);
      index = literal.end;
      skip();
      if (source[index] !== "|") {
        break;
      }
      index += 1;
    }
    while (index < source.length && !source.startsWith("=>", index)) {
      if (source[index] === '"') {
        index = readString(source, index).end;
        continue;
      }
      index += 1;
    }
    if (index >= source.length) {
      fail(`coverage: unterminated match arm after ${JSON.stringify(marker)}`);
    }
    index += 2;
    skip();
    keys.push(...patterns);
    index =
      source[index] === "{"
        ? skipBlock(source, index)
        : skipExpression(source, index);
  }
}

function checkCoverage(schema: Schema): boolean {
  const problems: string[] = [];
  const known = new Set<string>(LANGUAGES);
  for (const language of LANGUAGES) {
    if (!(language in schema.languages)) {
      problems.push(`schema.json is missing the ${language} language`);
    }
  }
  for (const language of Object.keys(schema.languages)) {
    if (!known.has(language)) {
      problems.push(`schema.json names unknown language ${language}`);
    }
  }

  let checked = 0;
  for (const crate of readdirSync(LANGUAGES_PATH)) {
    const file = path.join(LANGUAGES_PATH, crate, "src/config.rs");
    if (!existsSync(file)) {
      continue;
    }
    const keys = [...new Set(matchArms(readFileSync(file, "utf8"), "fn set("))];
    for (const language of SHARED_CONFIGS[crate] ?? [crate]) {
      const options = schema.languages[language];
      if (!options) {
        continue;
      }
      checked += keys.length;
      for (const key of keys) {
        if (!(key in options)) {
          problems.push(
            `${language}: ${crate}/src/config.rs accepts ${key}, schema.json does not`,
          );
        }
      }
      for (const key of Object.keys(options)) {
        if (!keys.includes(key)) {
          problems.push(
            `${language}: schema.json offers ${key}, ${crate}/src/config.rs rejects it`,
          );
        }
      }
    }
  }

  const core = readFileSync(CORE_PARSE_PATH, "utf8");
  const globalKeys = matchArms(core, "if global {");
  const blockKeys = matchArms(core, "fn parse(")
    .map((key) =>
      key === "include"
        ? "included_files"
        : key === "exclude"
          ? "excluded_files"
          : key,
    )
    .filter((key, at, all) => key !== "" && all.indexOf(key) === at);
  for (const key of globalKeys) {
    if (!(key in schema.global)) {
      problems.push(
        `global: core/config/parse.rs accepts ${key}, schema.json does not`,
      );
    }
  }
  for (const key of Object.keys(schema.global)) {
    if (!globalKeys.includes(key)) {
      problems.push(
        `global: schema.json offers ${key}, core/config/parse.rs rejects it`,
      );
    }
  }
  for (const key of blockKeys) {
    if (!(key in schema.files)) {
      problems.push(
        `files: core/config/parse.rs accepts ${key}, schema.json does not`,
      );
    }
  }
  for (const key of Object.keys(schema.files)) {
    if (!blockKeys.includes(key)) {
      problems.push(
        `files: schema.json offers ${key}, core/config/parse.rs rejects it`,
      );
    }
  }

  if (problems.length > 0) {
    console.error(`coverage failed:\n  ${problems.join("\n  ")}`);
    return false;
  }
  console.log(
    `coverage: ${checked} language keys, ${globalKeys.length} global keys and ${Object.keys(schema.files).length} file patterns verified`,
  );
  return true;
}

main();
