import type * as vscode from "vscode";

export const SECTION = "dotfmt";
export const CONFIG_NAME = "dotfmt.dotfile";

/** `--lang` and `--dialect` for untitled buffers; saved files are detected by name. */
export const LANGUAGES: Readonly<Record<string, readonly [string, string?]>> = {
  bibtex: ["bib"],
  chatagent: ["markdown"],
  dockercompose: ["yaml"],
  dotenv: ["env"],
  dotfile: ["conf"],
  ignore: ["ignore"],
  ini: ["ini"],
  instructions: ["markdown"],
  json: ["json"],
  jsonc: ["json", "jsonc"],
  jsonl: ["json", "jsonl"],
  latex: ["latex"],
  lua: ["lua"],
  markdown: ["markdown"],
  prompt: ["markdown"],
  properties: ["conf"],
  skill: ["markdown"],
  tex: ["latex"],
  toml: ["toml"],
  yaml: ["yaml"],
};

// Mirrors Language::for_path in dotfmt-core, for files VS Code gives another language.
const PATTERNS = [
  "**/*.{bib,conf,config,dotfile,env,gitignore,gitattributes,ignore,ini,service,timer,path,socket,target,mount,automount,swap,slice,scope,device,network,netdev,link,json,jsonc,hujson,jwcc,jsonl,ndjson,jsonlines,tex,ltx,latex,sty,cls,dtx,ins,def,clo,lco,fd,bbx,cbx,lbx,pgf,tikz,ctx,mkii,mkiv,mkvi,mkxl,lua,luau,md,markdown,mdown,mkd,plist,toml,yaml,yml}",
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
