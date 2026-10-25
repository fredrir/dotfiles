# LaTeX engine

Internal library used by `dotfmt -l latex`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l latex paper.tex chapter/
dotfmt -l latex --check .
dotfmt -e path/to/paper.tex < paper.tex
dotfmt -l latex --dialect context main.tex
dotfile dev check -p dotfmt-latex
```

| Behavior | Value |
| --- | --- |
| Engine | Embedded [tex-fmt](https://github.com/WGUNDERWOOD/tex-fmt) 0.5.7; no subprocess |
| Built-in files | `.tex`, `.ltx`, `.latex`, `.sty`, `.cls`, `.dtx`, `.ins`, `.def`, `.clo`, `.lco`, `.fd`, `.bbx`, `.cbx`, `.lbx`, `.pgf`, `.tikz`, `.ctx`, `.mkii`, `.mkiv`, `.mkvi`, `.mkxl` |
| Protected byte-for-byte | Verbatim environments, `\verb`, `\lstinline`, `\mintinline`, `\url`, `\path`, `\nolinkurl`, `\href`, `\input`, `\include`, `\includegraphics`, bibliography file arguments, `\directlua`/`\luadirect`/`\luaexec` blocks, escaped braces |
| Ignore comments | `% tex-fmt: skip`, `% tex-fmt: off`, `% tex-fmt: on` |
| Verification | `verify = true` formats a second time and requires the result to settle |
| Nesting limit | 100 levels; deeper input is rejected instead of overflowing the formatter |
| Encoding | UTF-8; an existing BOM is preserved; CRLF and CR are normalized, then written as configured |
| Configuration | `latex { ... }` in `dotfmt.dotfile`; global values and local overrides inherit |
| Deep nesting / stack | Buffer work runs on the 32 MiB worker stack |

Engine defaults below apply before configuration; see [shipped configuration](../../../../../../shared/tools/dotfmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `dialect` | `auto` | `auto`, `latex` (`pdftex`, `xelatex`, `lualatex`, `expl3`, `beamer`, `tikz`), `plain` (`tex`, `optex`), `context` (`ctx`, `mkiv`, `lmtx`) |
| `width` | `80` | 1–10000; preferred line width |
| `indent` | `2` | 1–16 columns per level |
| `indent_type` | `spaces` | `spaces`, `tabs` |
| `line_endings` | `unix` | `unix`, `windows` |
| `wrap` | `true` | Boolean; wrap code and comments at existing break characters |
| `wrap_min` | width − 10 | 1–10000; preferred minimum prefix before a break |
| `wrap_chars` | `" "` | Characters after which to break; a space is always added |
| `format_tables` | `false` | Boolean; align `tabular` rows |
| `blank_lines` | `1` | 0–10 consecutive blank lines kept outside protected content |
| `final_newline` | `false` | Boolean |
| `verify` | `false` | Boolean; format twice and require an identical result |
| `lists` | empty | Comma-separated extra list environments |
| `verbatims` | empty | Comma-separated extra verbatim environments |
| `no_indent_envs` | empty | Comma-separated extra environments that do not add indentation |

`width`, `indent` and `final_newline` also apply from the shared global block. `quote_style` is accepted there and ignored.

## Dialects

| Dialect | Behavior |
| --- | --- |
| `latex` | LaTeX2e/LaTeX3, pdfLaTeX, XeLaTeX, LuaLaTeX, Beamer and TikZ share this syntax |
| `plain` | Plain TeX/OpTeX; brace indentation is formatted, environment syntax is untouched |
| `context` | `\start...`/`\stop...` is mapped onto the environment engine and restored; `typing`, `TEX`, `MPcode`, `Lua` and `buffer` are protected |

`auto` selects `context` for `.ctx`, `.mkii`, `.mkiv`, `.mkvi` and `.mkxl`, and `latex` otherwise. CLI `--dialect` overrides the configuration.

## Limitations

`tex-fmt`'s indentation model is heuristic. Braces and brackets in ordinary text contribute indentation, so inputs with very deep delimiter nesting are rejected at the documented limit. `format_tables` handles `tabular`; other table environments are left alone and are never wrapped. `%` comments and protected inline constructs are recognized by this adapter before `tex-fmt` sees them.
