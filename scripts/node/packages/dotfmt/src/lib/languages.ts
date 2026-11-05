import type * as vscode from "vscode";

export const SECTION = "dotfmt";
export const CONFIG_NAME = "dotfmt.dotfile";

/** Language ids dotfmt can format; untitled buffers use the language, files use their name. */
export const LANGUAGES: Readonly<Record<string, readonly [string, string?]>> = {
  astro: ["html"],
  bibtex: ["bib"],
  chatagent: ["markdown"],
  css: ["css"],
  dockercompose: ["yaml"],
  dotenv: ["env"],
  dotfile: ["conf"],
  html: ["html"],
  ignore: ["ignore"],
  ini: ["ini"],
  instructions: ["markdown"],
  javascript: ["javascript"],
  javascriptreact: ["javascript"],
  json: ["json"],
  jsonc: ["json", "jsonc"],
  jsonl: ["json", "jsonl"],
  latex: ["latex"],
  less: ["css"],
  lua: ["lua"],
  markdown: ["markdown"],
  prompt: ["markdown"],
  properties: ["conf"],
  sass: ["css"],
  scss: ["css"],
  skill: ["markdown"],
  svelte: ["html"],
  tex: ["latex"],
  toml: ["toml"],
  typescript: ["typescript"],
  typescriptreact: ["typescript"],
  vue: ["html"],
  yaml: ["yaml"],
};

// Mirrors Language::for_path in dotfmt-core, for files VS Code gives another language.
const PATTERNS = [
  "**/*.{astro,bib,conf,config,dotfile,env,gitignore,gitattributes,ignore,ini,service,timer,path,socket,target,mount,automount,swap,slice,scope,device,network,netdev,link,json,jsonc,hujson,jwcc,jsonl,ndjson,jsonlines,tex,ltx,latex,sty,cls,dtx,ins,def,clo,lco,fd,bbx,cbx,lbx,pgf,tikz,ctx,mkii,mkiv,mkvi,mkxl,lua,luau,md,markdown,mdown,mkd,plist,toml,yaml,yml,ts,tsx,mts,cts,js,jsx,mjs,cjs,css,scss,sass,less,pcss,html,htm,xhtml,vue,svelte}",
  "**/{.env,env,.gitignore,.ignore,ignore,.prettierignore,.vscodeignore,.gitattributes,.gitconfig,.gitmodules,.editorconfig,ssh_config,sshd_config}",
  "**/.env.*",
  "**/{.git/info/exclude,.git/info/attributes,.git/config,git/config,.ssh/config,ssh/config}",
  "**/{.ssh,ssh}/config.d/**",
];

export const SELECTOR: vscode.DocumentFilter[] = [
  ...Object.keys(LANGUAGES).flatMap((language) => [
    { scheme: "file", language },
    { scheme: "untitled", language },
  ]),
  ...PATTERNS.map((pattern) => ({ scheme: "file", pattern })),
];
