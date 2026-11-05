import type * as vscode from "vscode";

export const SECTION = "dotfmt";
export const CONFIG_NAME = "dotfmt.dotfile";

/** Language ids dotfmt can format; untitled buffers use their VS Code language. */
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

// Mirrors Language::for_path in dotfmt-core, for files VS Code gives another language.
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
