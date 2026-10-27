Bun.build({
  entrypoints: ["src/extension.ts"],
  packages: "bundle",
  minify: true,
  sourcemap: false,
  external: ["vscode"],
  outdir: "./dot-code",
});
