import type * as vscode from "vscode";

export const SECTION = "dotfmt";
export const CONFIG_NAME = "dotfmt.dotfile";

export const LANGUAGES: readonly string[] = [
  "astro",
  "bibtex",
  "chatagent",
  "css",
  "dockercompose",
  "dotenv",
  "dotfile",
  "html",
  "ignore",
  "ini",
  "instructions",
  "javascript",
  "javascriptreact",
  "json",
  "jsonc",
  "jsonl",
  "latex",
  "less",
  "lua",
  "markdown",
  "prompt",
  "properties",
  "sass",
  "scss",
  "skill",
  "svelte",
  "tex",
  "toml",
  "typescript",
  "typescriptreact",
  "vue",
  "yaml",
];

const LANGUAGE_IDS: Record<string, readonly string[]> = {
  bib: ["bibtex"],
  conf: ["dotfile", "properties"],
  env: ["dotenv"],
  ignore: ["ignore"],
  ini: ["ini"],
  json: ["json", "jsonc", "jsonl"],
  latex: ["latex", "tex"],
  lua: ["lua"],
  markdown: ["markdown", "chatagent", "instructions", "prompt", "skill"],
  plist: ["plist", "xml"],
  toml: ["toml"],
  yaml: ["yaml", "dockercompose"],
  typescript: ["typescript", "typescriptreact"],
  javascript: ["javascript", "javascriptreact"],
  css: ["css", "scss", "sass", "less"],
  html: ["html", "vue", "svelte", "astro"],
};

export function enabledFormatterLanguages(
  enabled: readonly string[],
  available: readonly string[],
): string[] {
  const installed = new Set(available);
  const selected = new Set<string>();
  for (const name of enabled) {
    const ids = LANGUAGE_IDS[name] ?? [name];
    for (const id of ids) {
      if (installed.has(id)) {
        selected.add(id);
      }
    }
  }
  return [
    ...LANGUAGES.filter((language) => selected.has(language)),
    ...[...selected].filter((language) => !LANGUAGES.includes(language)),
  ];
}

const PATTERNS = [
  "**/*.{astro,bib,conf,config,dotfile,env,gitignore,gitattributes,ignore,ini,service,timer,path,socket,target,mount,automount,swap,slice,scope,device,network,netdev,link,json,jsonc,hujson,jwcc,jsonl,ndjson,jsonlines,tex,ltx,latex,sty,cls,dtx,ins,def,clo,lco,fd,bbx,cbx,lbx,pgf,tikz,ctx,mkii,mkiv,mkvi,mkxl,lua,luau,md,markdown,mdown,mkd,plist,toml,yaml,yml,ts,tsx,mts,cts,js,jsx,mjs,cjs,css,scss,sass,less,pcss,html,htm,xhtml,vue,svelte}",
  "**/{.env,env,.gitignore,.ignore,ignore,.prettierignore,.vscodeignore,.gitattributes,.gitconfig,.gitmodules,.editorconfig,ssh_config,sshd_config}",
  "**/.env.*",
  "**/{.git/info/exclude,.git/info/attributes,.git/config,git/config,.ssh/config,ssh/config}",
  "**/{.ssh,ssh}/config.d/**",
];

export const SELECTOR: vscode.DocumentFilter[] = [
  ...LANGUAGES.flatMap((language) => [
    { scheme: "file", language },
    { scheme: "untitled", language },
  ]),
  ...PATTERNS.map((pattern) => ({ scheme: "file", pattern })),
];
