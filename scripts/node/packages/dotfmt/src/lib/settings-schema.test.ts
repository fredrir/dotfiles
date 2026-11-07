import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "bun:test";

interface Option {
  enum?: unknown[];
  default?: unknown;
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
    configuration?: { properties?: Record<string, Record<string, unknown>> };
  };
}

const root = path.join(import.meta.dir, "../..");
const schema = JSON.parse(
  readFileSync(path.join(root, "schema.json"), "utf8"),
) as Schema;
const manifest = JSON.parse(
  readFileSync(path.join(root, "package.json"), "utf8"),
) as Manifest;
const settings = manifest.contributes?.configuration?.properties ?? {};

function generatedOptions(): { key: string; option: Option }[] {
  const options: { key: string; option: Option }[] = [];
  for (const [key, option] of Object.entries(schema.global)) {
    options.push({ key: `dotfmt.format.global.${key}`, option });
  }
  for (const [key, option] of Object.entries(schema.files)) {
    options.push({ key: `dotfmt.format.${key}`, option });
  }
  for (const [language, languageOptions] of Object.entries(schema.languages)) {
    const merged = { ...schema.common, ...languageOptions };
    for (const [key, option] of Object.entries(merged)) {
      options.push({ key: `dotfmt.format.${language}.${key}`, option });
    }
  }
  return options;
}

describe("generated format settings", () => {
  test("every schema enum reaches package.json unchanged", () => {
    for (const { key, option } of generatedOptions()) {
      const property = settings[key];
      expect(property, key).toBeDefined();
      expect(property?.default, key).toBeNull();
      if (option.enum) {
        expect(property?.enum, key).toEqual(option.enum);
      }
    }
  });

  test("JavaScript shares TypeScript's auto quote styles", () => {
    const javascript = schema.languages.javascript?.quote_style;
    const typescript = schema.languages.typescript?.quote_style;
    expect(javascript?.enum).toEqual(typescript?.enum);
    expect(javascript?.enum).toContain("auto-prefer-double");
  });

  test("markdown dialect accepts every alias its description names", () => {
    const dialect = schema.languages.markdown?.dialect;
    expect(dialect).toBeDefined();
    const named = [...dialect!.description.matchAll(/`([^`]+)`/g)].map(
      (match) => match[1],
    );
    for (const alias of named) {
      expect(dialect!.enum, alias).toContain(alias);
    }
    expect(dialect!.enum).toContain("github-flavored-markdown");
  });

  test("ini indent_type documents the dialect default, not spaces", () => {
    const indentType = schema.languages.ini?.indent_type;
    expect(indentType?.default).toBeUndefined();
    expect(indentType?.defaultDescription).toBe(
      "the dialect's own indentation",
    );
    const generated = settings["dotfmt.format.ini.indent_type"];
    expect(generated?.markdownDescription).toContain(
      "the dialect's own indentation",
    );
  });
});
