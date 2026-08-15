# The `.dotfile` Language

**`.dotfile` version:** 1

**Generated lock version:** 1

**Status:** current normative specification

**Repository:** `fredrir/dotfiles`

This specification defines one small, declarative language for the repository's package
requirements, profiles, hosts, theme inputs, deployment mappings, secret and system plans,
recipient keys, scan exceptions, and generated benchmark baselines. The compiler turns the
resolution domains into the committed `package.lock.dotfile` at the repository root.

The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative. Examples are normative
when they demonstrate syntax or an algorithm; comments in examples are explanatory.

## 1. Version declarations

`@dotfile-version = "1"` means “parse every repository-owned `.dotfile` source using version 1 of
the syntax and semantics in this specification.” It appears exactly once, as the first
non-comment entry of `config/profiles.dotfile`. There is no comma after it. An implementation MUST
reject a version it does not support instead of guessing how to interpret the file.

The generated lock has a separate `@lock-version = "1"` because authored source syntax and the
generated lock layout can evolve independently. Users write the source declaration; the compiler
copies that value into the lock and writes `@lock-version`. The lock also records
`@builtins-version`, which identifies the exact built-in adapter, inference, default, deployment,
observation, application, and provenance rules used to compile and consume it. These are opaque
version identifiers, not quantities; `"1"` is quoted because `.dotfile` has no numeric literals and
no version arithmetic. A consumer MUST reject an unsupported lock or built-ins version instead of
attempting a partial read.

The authored declaration is the only version selector. `.dotfile` version 1 selects exactly
generated-lock version 1 and built-ins version 1; another implementation processing the same
source version MUST NOT silently select a different tuple. A source-visible change to accepted
managers, installers, defaults, or other built-in semantics therefore requires a new `.dotfile`
version as well as any affected generated-lock or built-ins version.

All configuration interpreted by the `dotfile` tool uses `.dotfile` syntax. Native configuration
that is merely deployed to another program—such as Cargo manifests or Starship configuration—stays
in the format required by that program.

## 2. Design goals and non-goals

`.dotfile` version 1 is designed to be:

- **declarative:** source states facts, demands, profiles, and mappings, never procedures;
- **hermetic at compile time:** identical declared repository inputs produce identical IR and
  lock bytes;
- **explainable:** every resolved value, demand, and deployment row retains provenance;
- **conflict detecting:** incompatible facts fail instead of depending on incidental file order;
- **safe to apply:** planning, collision detection, ownership checks, and privilege boundaries are
  specified separately from compilation;
- **small:** the syntax is intentionally narrower than Nix, Dhall, CUE, Jsonnet, Nickel, or HCL;
- **behavior-preserving:** repository-visible mappings and operations have explicit conformance
  coverage.

`.dotfile` version 1 is not:

- a shell language;
- a system-provisioning language;
- a general package solver;
- an import or templating framework for arbitrary remote code;
- a way to make compilation depend on the current host, environment, installed software, clock,
  locale, network, or process state.

Package lists describe software required by this repository's configuration. Base systems,
drivers, kernels, and unrelated workstation provisioning remain out of scope.

## 3. Processing model

The processing model has four operations.

1. **Parse** source bytes into a concrete syntax tree, preserving comments and spans.
2. **Compile** all semantic source domains into a qualified, occurrence-based IR. Compilation is
   pure and fully validates every declared profile.
3. **Bind** one profile, group-variant selection, invoking user, home/XDG anchors, host, and actual
   target filesystem. Machine observations do not enter the committed lock.
4. **Apply or inspect** the bound plan. Only this operation observes installed tools, services, fonts,
   destinations, vault identities, or destination-volume behavior.

Compilation MUST NOT read environment variables, the clock, hostname, PATH, installed programs,
network, or random state. It MAY read only the repository inputs defined in sections 4 and 16–18.
It MUST enumerate all inputs in a specified order and fully validate exported IR; an
implementation may evaluate lazily internally only if no error can remain hidden.

The generated lock records resolved identities, facts, demands, and deployment plans; it is not a
content snapshot. Ordinary linked file contents remain live in the working tree. Copied,
encrypted, and template source identities are fingerprinted because their bytes are materialized.

## 4. Files and domains

The `.dotfile` version is declared once in `config/profiles.dotfile`. Every repository-owned
`.dotfile` source is parsed with that version.

The first non-comment entry of `config/profiles.dotfile` MUST be exactly
`@dotfile-version = "1"`; it cannot use a binding, interpolation, soft line break, or trailing
comma. The bootstrap reader recognizes only this ASCII preamble before selecting the lexer/parser
for that version. `@dotfile-version` is invalid in every other source file, and duplicate
declarations are errors.

| Path | Domain | Lock input | Owner |
|---|---|---:|---|
| `config/profiles.dotfile` | groups, profiles, repo defaults | yes | human |
| `config/hosts.dotfile` | hosts and hardware facts | yes | human |
| `<group-directory>/package.dotfile` | group demands and fact extensions | yes | human |
| `<group-directory>/<package>/package.dotfile` | facet, demands, facts, deployment | yes | human |
| `<group-directory>/overrides/<variant>/<package>/package.dotfile` | variant deployment metadata | yes | human/tool |
| `config/keys.dotfile` | age recipients | no; separate consumer | human/tool |
| `config/scan.dotfile` | secret-scan allow rules | no; separate consumer | human |
| `benchmarks/baselines.dotfile` | benchmark baselines | no; separate generated domain | tool |
| `theme/roles.dotfile` | shared theme role definitions | no; theme consumer | human |
| `theme/fonts.dotfile` | shared theme font definitions | no; theme consumer | human |
| `theme/maps/*.dotfile` | registered application theme maps | no; theme consumer | human |
| `theme/profiles/*.dotfile` | theme definitions | structure/name only | human |
| `vars.enc.yaml` | fixed encrypted template-variable store | structure/identity only | vault |
| `package.lock.dotfile` | generated lock | generated | compiler |

`config/profiles.dotfile` is parsed and validated first. It supplies the group directory map used
to discover the remaining semantic sources. A missing mandatory semantic file is an error. A
missing group-root `package.dotfile` is equivalent to an empty one.

`vars.enc.yaml` is mandatory when at least one template leaf exists and otherwise may be absent.
It MUST be a tracked regular file. Compilation reads and hashes its ciphertext bytes but never
decrypts it.

The keys, scan, benchmark, and theme-definition files share the lexer, grammar, CST, and formatter,
but their specialized consumers read typed source directly. Their contents are deliberately not
duplicated into the generated lock. Theme profile paths additionally supply its theme-name
inventory. The statement “commands read the lock” means that graph, profile, package-coordinate,
and deployment consumers use the lock as their only resolution source; it does not apply to the
compiler, formatter, editor, theme generator, vault, scanner, or benchmark store.

## 5. Namespaces and qualified identity

The resolver never searches one undifferentiated name pool.

| Namespace | Source key | Canonical IR/CLI identity |
|---|---|---|
| group | globally unique `IDENT` | `group:<name>` |
| profile | globally unique `IDENT` | `profile:<name>` |
| facet | group plus logical package name and optional variant | base `facet:<group>/<package>`; variant `facet:<group>/<package>@<variant>` |
| entity | global `IDENT` | `entity:<name>` |
| resource | resource kind plus key | `resource:<kind>/<key>` |
| facet path | facet identity plus normalized relative source path | base `path:<group>/<package>/<path>`; variant `path:<group>/<package>@<variant>/<path>` |
| host | block name | `host:<name>` |
| theme | `theme/profiles/<name>.dotfile` basename | `theme:<name>` |
| binding | lexical binding name | never serialized as a node |

All identities are case-sensitive except hostname aliases, which are ASCII-case-insensitive.
The `@<variant>` facet suffix is an IR/CLI spelling only; variant source location remains the
directory form in section 19. Because every assertion and mapping stores the fully qualified facet
ID, base and variant path claims cannot collide in the lock namespace.
Source positions are typed by their schema:

- a dependency name creates or refers to an entity;
- a resource `@key` creates or refers to a key within its resource kind;
- `@groups` values refer to groups;
- `@profile` refers to a profile;
- `@theme` refers to an existing theme;
- `$name` and `${name}` refer only to lexical string bindings;
- `@extend entity/name` and `@extend kind/key` refer to an already-declared qualified node.

An unqualified query such as `dotfile why wezterm` is accepted only when it resolves uniquely.
An ambiguous query MUST fail and list accepted qualified spellings.

Entity names form an open namespace: first use in dependency position declares the identity.
Consequently, an entity typo cannot be an “unresolved reference.” The compiler SHOULD warn when a
new entity name is unusually close to an existing entity, package coordinate, or facet name.

## 6. Lexical structure

### 6.1 Encoding and line endings

- Input MUST be strict UTF-8.
- One UTF-8 BOM is accepted only at byte offset zero and discarded. A BOM elsewhere is invalid.
- LF and CRLF are accepted as `NEWLINE`; bare CR is invalid.
- NUL and literal C0/C1 control characters are invalid outside strings. Space and TAB are the only
  horizontal whitespace.
- Horizontal whitespace is insignificant between tokens. Indentation has no meaning.
- The formatter emits UTF-8 without a BOM, LF only, and exactly one final LF for a non-empty file.
  An empty file formats to zero bytes.

### 6.2 Comments

`#` starts a comment outside a string only when it is the first non-horizontal-whitespace
character on a line or immediately follows horizontal whitespace. It consumes through the byte
before newline or EOF.

These are lexical fragments, not a complete file:

```text
wezterm                    # valid trailing comment
"# literal string data"
```

`wezterm#comment` is a lexical error, not a comment. There are no block comments.

### 6.3 Identifiers and bindings

```ebnf
ALPHA       = "A" … "Z" | "a" … "z" ;
DIGIT       = "0" … "9" ;
ID_CONT     = ALPHA | DIGIT | "_" | "." | "+" | "-" ;
WORD        = (ALPHA | DIGIT | "." | "_"), { ID_CONT } ;
```

The lexer emits one `WORD` token; it does not emit competing `IDENT` and `BINDING` token kinds.
In an identifier position the parser validates
`(ALPHA | DIGIT | "."), { ID_CONT }`; in a binding position it validates
`(ALPHA | "_"), { ALPHA | DIGIT | "_" }`. A word may satisfy both because the position supplies
the type.

Additional rules:

- `IDENT` is ASCII, case-sensitive, and MUST NOT equal `.` or `..`.
- Digit-leading identifiers are intentional: `7z` is valid.
- `.zshrc`, `fc-cache`, `wezterm-mux`, and `hack_nerd_font` are valid.
- `@`, `$`, and `?` are separate tokens and MUST be adjacent to their subject.
- `PATHREF` is one terminal beginning with `./`. The lexer recognizes the complete bare or quoted
  path token before considering a `.`-leading `WORD`; `./` is not emitted as a separate token.
- `@let` and `@extend` are the only compound keyword tokens. They require exact spelling, no space
  after `@`, at least one horizontal space after the keyword, and a following word. Thus
  `@letfoo = "x"` is the ordinary attribute `letfoo`, while `@let = "x"` is a malformed binding
  declaration.
- `if`, `then`, `else`, `for`, `in`, `import`, `as`, `null`, `true`, and `false` are reserved for a
  future `.dotfile` version and are invalid as unquoted identifiers in version 1.
- Qualified IR strings containing `:`, `/`, or both are not source identifiers.

`.dotfile` version 1 has no numeric or Boolean literal. Versions, modes, counts, dates, hashes,
theme sizes/alpha values, and other data are schema-checked strings. This preserves the rule that
every bare value token is a typed reference.

### 6.4 Strings and interpolation

Strings are double-quoted Unicode scalar sequences. Literal newlines and literal control
characters are invalid.

```ebnf
HEX           = DIGIT | "A" … "F" | "a" … "f" ;
ESCAPE        = '\\"' | '\\\\' | '\\n' | '\\r' | '\\t' | '\\b' | '\\f' | '\\$'
              | '\\u{' HEX { HEX } '}' ;
INTERPOLATION = '${' binding '}' ;
STRING        = '"', { STRING_CHAR | ESCAPE | INTERPOLATION }, '"' ;
```

`STRING_CHAR` is any Unicode scalar except `"`, `\`, CR, LF, a literal C0/C1 control, or a `$`
immediately followed by `{`. A `$` in every other position is an ordinary string character.

`\u{...}` contains one to six hexadecimal digits and MUST denote a Unicode scalar value,
not a surrogate or a value above U+10FFFF. Any unlisted escape is an error. `$` is literal unless
it begins `${...}`.

Canonical source and lock strings decode first, then emit `\"`, `\\`, `\n`, `\r`, `\t`, `\b`,
and `\f` for those exact scalars; `\$` only for a literal `$` immediately followed by `{`;
`\u{<lowercase-hex>}` with no leading zero for every other C0/C1 control; and every other scalar
directly as UTF-8. An input Unicode escape may use either hex case and one to six digits, but the
formatter never preserves that alternative spelling.

Interpolation accepts bindings only, never general expressions:

```dotfile
@let vault = "~/Documents/main"
@destination = "${vault}/.obsidian"
```

The noncanonical multi-atom form is accepted as input sugar on one physical line:

```dotfile
@destination = $vault "/.obsidian"
```

It desugars to the interpolated string above, which is the formatter's canonical form.
Interpolation performs no path read, environment lookup, command substitution, or implicit value
conversion.

### 6.5 Source path references

Bare source paths cover common names, including `%`; quoted source paths cover existing repository
names with spaces, Unicode, or punctuation outside `PATH_SAFE`.

```ebnf
PATH_SAFE    = ALPHA | DIGIT | "_" | "." | "+" | "-" | "%" | "@" | "=" ;
PATH_SEG     = PATH_SAFE, { PATH_SAFE } ;
BARE_PATH    = "./", PATH_SEG, { "/", PATH_SEG } ;
QUOTED_PATH  = "./", STRING ;
PATHREF      = BARE_PATH | QUOTED_PATH ;
```

There is no whitespace between `./` and a quoted string:

```dotfile
./.zshrc
./"presets/Mocha Islands/settings.json"
```

After decoding, a path reference MUST be non-empty and relative, contain no empty, `.` or `..`
component, contain no backslash or control character, and have no leading or trailing slash.
The `STRING` in `QUOTED_PATH` is contextually required to contain no interpolation. A filename
such as `foo..bar` is valid because `..`
is rejected as a complete component, not as a substring.

The formatter uses the bare form when every component fits `PATH_SEG`; otherwise it uses the
quoted form.

### 6.6 Destination and machine paths

Path-typed string expressions MUST resolve to one of:

- `~` or `~/...`, anchored to the invoking user's home during machine binding; or
- `/...`, an absolute POSIX path.

They MUST NOT contain `~user`, `$HOME`, a non-leading `~`, backslash, control characters, empty,
`.` or `..` components, a trailing slash, or a leading `//`. `/` itself is not a valid leaf
destination. The source spelling is retained; paths are not silently rewritten.

The lock stores the symbolic home anchor, not the compiler machine's absolute home. Source and
destination path strings MUST be NFC; canonical-equivalent but differently encoded spellings are
rejected rather than normalized silently.

## 7. Token-level grammar

The lexer excludes horizontal whitespace and comments from the significant-token stream, retains
physical `NL` tokens, and also preserves every whitespace/comment byte in an ordered trivia gap
between adjacent significant tokens. The CST owns those gaps directly; nullable grammar symbols
do not compete for trivia. `STRING` is the token from section 6.4. Contextual schemas assign
meaning after this generic syntax parses.

```ebnf
file            = newlines,
                  [ entry, { separator, entry } ],
                  newlines, EOF ;

newlines        = { NL } ;
one_or_more_nl  = NL, { NL } ;
soft_break      = { NL } ;

separator       = newlines, ",", newlines
                | one_or_more_nl ;
trailing_comma  = newlines, ",", newlines ;

entry           = let_decl
                | extend_entry
                | attribute
                | sigil_block
                | named_entry
                | path_entry ;

let_decl        = AT_LET, binding, "=", soft_break, string_expr ;
extend_entry    = AT_EXTEND, qualified_ref, block ;
qualified_ref   = ident, "/", ident ;
attribute       = "@", ident, "=", soft_break, value ;
sigil_block     = [ "?" ], "@", ident, block ;
named_entry     = [ "?" ], ident,
                  [ "=", soft_break, value | block ] ;
path_entry      = [ "?" ], PATHREF, [ block ] ;

block           = "{", body, "}" ;
body            = newlines,
                  [ entry,
                    { separator, entry },
                    [ trailing_comma ] ],
                  newlines ;

value           = string_expr | reference | list ;
reference       = ident ;
string_expr     = string_atom, { string_atom } ;
string_atom     = STRING | VARREF ;
VARREF          = "$", binding ;

list            = "[", newlines,
                  [ value,
                    { newlines, ",", newlines, value },
                    [ trailing_comma ] ],
                  newlines, "]" ;
```

`ident` and `binding` are contextual validations of `WORD` from section 6.3. `AT_LET` and
`AT_EXTEND` are the compound keyword tokens defined there. Each run of `NL` in an empty file,
block, or list occupies its one physical trivia/token gap; the grammar's nullable `newlines`
notation does not create alternative CST ownership.

Consequences:

- `brew =` followed by newline and `"homebrew"` is valid; a soft break is permitted immediately
  after `=`.
- A string-expression concatenation cannot cross a physical newline.
- In the multi-atom form, consecutive `string_atom`s MUST have at least one horizontal
  whitespace character between their source spans. Thus `$vault "/x"` is accepted but
  `$vault"/x"` is not. The formatter always emits one interpolated `STRING`.
- Lists require commas; newlines alone never separate list values.
- Blocks permit comma or newline separators and an optional trailing comma.
- A top-level trailing comma is invalid.
- `?@font` is syntactically valid; a domain schema rejects `?` before structural blocks such as
  `@groups`.
- `{...}` is not a first-class value in `.dotfile` version 1. Structured data uses schema-defined
  blocks.
- There are no operators and therefore no expression-precedence table beyond string
  interpolation/concatenation, assignment, the declaration prefix `?`, and separators.

## 8. Values, references, and bindings

The generic value model is deliberately small:

```text
String
List<Value>
Reference(namespace supplied by schema position)
```

There is no `null`. Absence means that no value was contributed. Lists are ordered and homogeneous
according to their schema position. They never merge or concatenate implicitly.

The quoting rule is absolute:

> A bare value is a typed reference. A quoted value is data.

For example, `@theme = mocha` resolves the theme named `mocha`, while `role = "desktop"` is data.
A bare token in a string-only position is an error, even if a string with the same spelling would
be valid.

Bindings are weak, file-local string macros. The file root is a lexical scope exactly like a
block: its prologue ends at its first non-`@let` entry, and every nested block sees and may shadow
its bindings.

1. A block may start with an `@let` prologue.
2. Once any non-`@let` entry appears in that block, a later `@let` is a schema error.
3. Declarations evaluate in source order and see outer bindings plus earlier declarations in the
   same prologue.
4. A binding starts after its initializer. Self-reference and use-before-declaration are errors.
5. An inner block may shadow an outer binding; same-block redeclaration is an error.
6. Initializers are string expressions only.
7. Bindings never cross files and never become graph or lock nodes.
8. `~` is expanded only when the finished string is consumed by a path-typed field.

The formatter preserves binding source order and never hoists a declaration across another entry.

## 9. Requirement-domain forms and sugar

### 9.1 Facet files

A facet `package.dotfile` may contain, at its root:

- an `@let` prologue;
- facet attributes;
- entity demands;
- resource demands;
- fact-only `@extend` blocks;
- facet-local path nodes.

A group-root `package.dotfile` may contain bindings, entity/resource demands, group theme defaults,
and `@extend` blocks. Facet and path/deployment attributes are invalid there.

### 9.2 Entity demands

```dotfile
lazygit
?ncdu
rg = "ripgrep"
?gh = "github-cli"
wezterm {
    @version = "20260813-114614-18a44cb7"
    hammerspoon
}
```

The forms desugar as follows:

| Source | Core meaning |
|---|---|
| `name` | required demand occurrence targeting `entity:name` |
| `?name` | optional demand occurrence targeting `entity:name` |
| `name = "pkg"` | `name { @pkg = "pkg" }` |
| `?name = "pkg"` | optional occurrence plus the same fact contribution |
| `name { ... }` | occurrence, facts about `name`, and nested reason occurrences |

Assignment sugar accepts a string expression only. It never means aliasing or equality.

A facet never creates an implicit entity or self-demand. A config-only facet may be empty. A facet
that requires its same-named tool writes it explicitly.

### 9.3 Resource demands

`.dotfile` version 1 registers one resource kind, `font`:

```dotfile
@font {
    @key = hack_nerd_font
    @family = ["Hack Nerd Font Mono", "JetBrainsMono Nerd Font"]
}
```

The block creates a demand occurrence for `resource:font/hack_nerd_font` and contributes its facts.
`?@font` creates an optional resource occurrence. Every resource block MUST have exactly one direct,
bare `@key`. A string, interpolated key, duplicate key, or `@key` outside a resource is invalid.

Other resource kinds require new `.dotfile` and lock versions. Services and machine paths remain
entities with service/path check shapes in version 1.

### 9.4 Fact-only extension

Normal entity and resource blocks always create demands. Platform coordinates that do not create a
new recommendation use a qualified extension:

```dotfile
@extend font/hack_nerd_font {
    @pkg = "font-hack-nerd-font"
    @installer = "brew-cask"
}

@extend entity/wezterm {
    @pkg = "wezterm"
}
```

An extension contributes facts in its declaration group but no occurrence. Its target MUST be
declared by at least one normal demand/resource block somewhere in the compiled repository.
Extension bodies allow bindings followed by fact attributes only—no demands, path nodes, resource
blocks, or deployment attributes.

### 9.5 Path nodes

Path nodes are facet-local and valid only at a facet root:

```dotfile
./.zshrc { @destination = "~/.zshrc" }
./types { @deploy = "none" }
?./generated-report { @deploy = "none" }
```

A path node may:

- add or override a deployment mapping for its exact source prefix;
- override inherited deployment properties for its subtree; or
- with `@deploy = "none"`, declare a repository-path assertion without deployment.

Its body permits only an `@let` prologue followed by path deployment/assertion attributes; nested
demands, resources, extensions, and path nodes are invalid. An authored path node whose effective
action is not `none` MUST component-prefix-match at least one selected payload leaf. This catches a
misspelled deployable path at compile time. A facet-root implicit mapping may remain empty for a
valid config-only facet. Two path entries in one facet or variant facet may not decode to the same
path; duplicate prefix contributions are errors rather than a merge order.

A required check-only path is a `check` failure when absent. An optional check-only path is a
warning when absent. `@expect` selects the exact object type; `"any"` accepts any object returned
by `lstat`, while the other values require a regular file, ordinary directory, or symlink. The
check performs one no-follow observation of the named path and never scans descendants. An
assertion from a base facet is evaluated when its declaration group is active in the bound
profile. An assertion whose qualified facet carries `@<variant>` is evaluated only when that same
variant is selected for the active group; every unselected variant assertion is dormant.
Check-only path existence is not a compile input, allowing ignored/generated development trees
such as `shared/wezterm/types` to remain machine-local.

An optional path with an effective deployment other than `none` is invalid because it would make
the committed plan depend on whichever untracked files happen to exist on the compiler machine.

## 10. Demand occurrences, optionality, and cycles

The compiler does not merge identities while parsing nested demands. It first creates occurrences:

```text
DemandOccurrence {
    id                  stable source-derived occurrence ID
    target              qualified entity or resource ID
    root                qualified facet or group ID
    parent              optional parent occurrence ID
    declaration_group   group in which the source is located
    reason_chain        ordered qualified targets from root child to parent
    local_mode          required | optional
    effective_mode      required | optional
    source_span         file, byte range, start/end line and column
}
```

The root context starts required. For each occurrence:

```text
effective_required = local_required AND parent_effective_required
```

Equivalently, an occurrence is optional when it or any enclosing occurrence has `?`.

```dotfile
?kitty {
    xremap
}
```

creates:

```text
kitty:  local optional, effective optional
xremap: local required, effective optional, reason_chain [entity:kitty]
```

If `kitty` is required elsewhere, this particular `xremap` occurrence remains optional. Only after
filtering occurrences to a profile does the compiler fold by target identity:

```text
required  if any active occurrence is effectively required
optional  otherwise, if at least one active occurrence exists
absent    otherwise
```

Facts contributed inside an optional occurrence are not optional and are not suppressed. This is
the precise meaning of “facts merge; demands accumulate.”

For each declared profile, the compiler projects nested occurrence parents into an identity demand
graph. Every self-cycle or strongly connected component larger than one is an error. The diagnostic
MUST show the entire cycle and all contributing spans. A cycle that cannot be active in any declared
profile is harmless and is not in that profile's graph.

The occurrence tree, not the globally merged identity graph, is the source of `why` provenance.

## 11. Facts and profile-scoped merging

Every fact contribution records its qualified target, attribute, value, declaration group, and
source span before merging.

`.dotfile` version 1 has five merge classes:

| Class | Rule |
|---|---|
| identity | declares or selects a qualified identity; never merged as data |
| scoped scalar | one structurally equal value per group scope; descendant group overrides ancestor |
| scoped list | same as scoped scalar, but the whole ordered list is one value |
| facet-local | belongs to one facet or variant facet; never crosses facets |
| claim | resolved only by source-prefix deployment rules |
| lexical | bindings; not part of semantic merging |

`shared` is the distinguished root group and is active in every profile. For fact resolution only,
it is the virtual ancestor of every non-`shared` group, regardless of those groups' declared roots.
Its scoped facts are therefore a baseline that any active non-`shared` contribution may replace.
For one attribute and profile:

1. Gather contributions from active groups.
2. Contributions in the same group MUST be structurally equal; otherwise report all conflicting
   spans.
3. Remove any contribution that has an active descendant contribution.
4. If one maximal contribution remains, use it.
5. If several sibling maxima remain and are equal, use the value and retain all provenance.
6. If several sibling maxima disagree, the profile is invalid. Profile group order does not break
   fact conflicts.

Lists are compared as ordered lists. They do not union, concatenate, or merge element-wise.

`.dotfile` version 1 has no clear/unset/tombstone operation. Absence means inherit. If a profile genuinely
needs a different semantic shape, it contributes a replacement in a descendant group or uses a
separate identity. `@unset` is reserved for a future `.dotfile` version.

## 12. Attribute vocabulary

Every attribute and schema field is single-valued within its immediate semantic block unless a
domain explicitly says the entry is repeatable. Repeating one—even with an equal value—is a
schema error. Repeated demands and fact contributions in separate entries remain legal because
their occurrence/merge rules are explicit; this rule specifically prevents last-wins facet/path,
profile, host, and resource attributes.

### 12.1 Entity and resource facts

| Attribute | Type | Merge | Meaning |
|---|---|---|---|
| `@pkg` | string expression | scoped scalar | package name when it differs from the node's default |
| `@installer` | string | scoped scalar | explicit installer adapter, such as `brew-cask`, `aur`, `uv`, or `cargo` |
| `@bin` | command-name string | scoped scalar | PATH basename when it differs from the entity name |
| `@check` | string enum | scoped scalar | `command`, `package`, `font`, `service`, `path`, or `none` |
| `@version` | string | scoped scalar | expected version according to the selected check adapter |
| `@family` | string or list of strings | scoped list | preferred font-family stack; scalar normalizes to one-element list |
| `@service` | string | scoped scalar | service/unit label |
| `@scope` | string enum | scoped scalar | `user` (default) or `system`; valid only with service shape |
| `@path` | path-typed string expression | scoped scalar | machine path checked for existence; entity only |
| `@description` | string | scoped scalar | one-line node description |
| `@key` | bare reference | identity | resource identity; resource block only |

An attribute unknown to the current domain, valid only on another node kind, or inconsistent with
another resolved shape is an error. Descriptions MUST decode to one line.

### 12.2 Facet and path deployment attributes

| Attribute | Type | Default | Meaning |
|---|---|---|---|
| `@destination` | path-typed string expression | facet default only | exact destination root for the source prefix |
| `@deploy` | `"link"`, `"copy"`, `"none"` | `"link"` | deployment action for plain leaves |
| `@privilege` | `"user"`, `"system"` | `"user"` | user or privileged destination |
| `@sensitivity` | `"public"`, `"private"` | `"public"` | visibility and permission policy for plain leaves |
| `@mode` | four-digit octal string | section 20 | materialized file mode |
| `@owner` | string | none | required for system copies |
| `@group` | string | none | required for system copies |
| `@expect` | `"any"`, `"file"`, `"directory"`, `"symlink"` | `"any"` | exact-object assertion; valid only on a check-only path |
| `@description` | one-line string | none | facet description |
| `@theme` | theme reference | inherited | facet theme override |

These properties are facet-local. Deployment properties inherit from the facet or nearest ancestor
path node and may be replaced for a subtree. `@expect` is valid only on the exact check-only path
that declares it and does not inherit. None of these are entity facts or participate in entity
merge rules.

Validation rules include:

- `system` privilege requires an effective materialized action, an absolute destination outside
  home, and explicit `@owner` and `@group`; a plain leaf additionally requires explicit `@mode`
  after inheritance, while a transformed leaf gets its locked private mode from section 20.2;
- `link` with system privilege is invalid;
- `none` is an absolute subtree exclusion: it produces no deployment row, including for a
  transformed filename. `@destination`, ownership, mode, and sensitivity attributes on an
  effective `none` node are invalid; `@expect` remains valid;
- a plain leaf under effective `private` sensitivity is invalid; private source sets are sealed and
  contain only recognized transformed leaves;
- setuid, setgid, and sticky bits are invalid;
- `@mode`, `@owner`, and `@group` are invalid for links;
- a filename-derived `sops` or `template` render materializes as a private copy inside effective
  `link` or `copy`, but never overrides effective `none`; no source attribute can declassify it.

### 12.3 Group, profile, host, and lexical attributes

| Context | Attributes |
|---|---|
| group declaration | `@directory`, `@os`, `@arch`, `@description` |
| profile declaration | `@groups`, `@manager`, `@os`, `@arch`, `@theme`, `@description` |
| group-root package file | `@theme`, `@let` |
| facet | deployment attributes, `@description`, `@theme`, `@let` |
| host | `@profile`, `@theme` plus schema fields |
| any allowed block prologue | `@let` |

`@destination`, `@path`, and `@directory` are deliberately different names and types.

## 13. Groups and profiles

Groups are declared exactly once in `config/profiles.dotfile`. Names are globally unique even when
their declarations are nested. Nesting means semantic ancestry, not an inferred filesystem path.

```dotfile
@dotfile-version = "1"

@groups {
    shared { @directory = "shared" }
    macos {
        @directory = "macos"
        @os = "darwin"
    }
    linux {
        @os = "linux"

        common { @directory = "linux/common" }
        arch {
            @directory = "linux/arch"

            kde { @directory = "linux/kde" }
            hyprland { @directory = "linux/hyprland" }
        }
        ubuntu {
            @directory = "linux/ubuntu"

            server { @directory = "linux/server" }
        }
    }
}

@profiles {
    macos {
        @groups = [macos]
        @manager = "brew"
        @os = "darwin"
    }
    kde {
        @groups = [common, kde]
        @manager = "pacman"
        @os = "linux"
    }
    hyprland {
        @groups = [common, hyprland]
        @manager = "pacman"
        @os = "linux"
    }
    kde-hyprland {
        @groups = [common, kde, hyprland]
        @manager = "pacman"
        @os = "linux"
    }
    server {
        @groups = [server]
        @manager = "apt"
        @os = "linux"
    }
}

@theme = mocha
```

`shared` MUST be declared, MUST have a directory, MUST be a declared root group, and is always
active. It is omitted from profile lists. In the semantic ancestry used for facts and serialized
`ancestors`, `shared` is additionally the first virtual ancestor of every other group; this does
not change any group's declared `parent` or filesystem location. Other groups may be abstract and
omit `@directory`; abstract groups can carry constraints and ancestry but contain no facets.

Activating a group activates all of its ancestors. It does not activate descendants or siblings.
This is why `common` is a sibling of the Arch/Ubuntu branches rather than the concrete directory of
an abstract `linux` ancestor: the server profile must not acquire desktop/common facets.

For each item in a profile's `@groups` list, ancestors are inserted immediately before the item;
duplicates retain their first position. `shared` is then prepended. For the `kde` example, the
active order is:

```text
[shared, linux, common, arch, kde]
```

The order is meaningful only for cross-group deployment overlays and presentation. It never
silently resolves fact conflicts.

Each profile MUST declare exactly one default `@manager`; it is not inherited from a group. This
makes shared dependencies unambiguous and prevents Ubuntu from accidentally inheriting Pacman.
The built-in manager-to-default-installer mapping is:

| Manager | Default installer |
|---|---|
| `brew` | `brew-formula` |
| `pacman` | `pacman` |
| `apt` | `apt` |

`@manager` names the profile's native package-management context; `@installer` names the concrete
emitter/adapter for one resolved node. Additional source-visible managers or installers require a
new `.dotfile` version and built-ins version; the generated-lock version changes only if their
serialized representation changes. A node's `@installer` overrides the profile default.

The versioned adapter registry declares each installer's supported profile OS and native managers.
For example, `brew-formula` and `brew-cask` require manager `brew` on Darwin, `aur` requires
`pacman` on Linux, and `apt` requires manager `apt` on Linux. Cross-manager installers such as
`cargo` or `uv` are permitted only where their registry entry explicitly supports the profile.
An otherwise known but incompatible installer is a compile error for that profile.

Each profile MUST declare scalar `@os` as `"darwin"` or `"linux"`. A group's `@os`, when present,
has the same scalar type and MUST equal the profile OS whenever that group is active.

`@arch` on either a profile or group is an optional, non-empty ordered list containing unique
canonical strings from `"x86_64"` and `"aarch64"`; absence means both. For each profile, intersect
its allowed set with every active group's set. An empty intersection is a compile error. At bind
time, OS-reported `amd64` normalizes to `x86_64` and `arm64` to `aarch64`; no other alias is
accepted. The bound machine's OS and normalized architecture MUST belong to the profile's final
intersection. Incompatibility is an error, not a warning.

Group directories are normalized repository-relative paths with no empty, `.` or `..` component.
They MUST exist as ordinary directories, MUST NOT be symlinks, MUST be unique, and MUST NOT equal or
contain one another. A directory-bearing group's immediate subdirectories are package candidates,
except the reserved `overrides` directory.

## 14. Hosts and themes

`config/hosts.dotfile` contains one block per physical machine:

```dotfile
archie {
    hostnames = ["archpc", "archie", "archie.local"]
    role = "desktop"
    @profile = kde-hyprland
    @theme = mocha

    CPU_COOLER = "Noctua NH-D15"
    MEMORY = "Corsair CMK32GX5M2B6000Z30 32 GB (2×16 GB) DDR5-6000 CL30"
}
```

Rules:

- The block name is a stable machine ID and is also an implicit hostname alias.
- `hostnames` is a non-empty ordered list of strings. Aliases are unique after ASCII lowercase and
  removal of one terminal DNS dot.
- `role` is one of `"desktop"`, `"laptop"`, or `"server"`.
- `@profile` is required and resolves a profile.
- `@theme` is optional and resolves a theme.
- Extension fact keys match `[A-Z][A-Z0-9_]*`; values are strings or homogeneous lists of strings.
- Duplicate host blocks or duplicate fields are errors; unknown lowercase standard fields are
  errors rather than ignored.

Profile selection precedence is explicit CLI profile, saved machine state, then matched host.
Read-only `check` reports saved/host disagreement. A deployment-applying command MUST refuse that
disagreement unless the user supplied an explicit profile for this invocation; a successful
explicit apply may update saved state.

Machine state remains outside the repository at `$XDG_CONFIG_HOME/dotfile`, falling back to
`~/.config/dotfile`. `profile` is exactly one unqualified profile name plus LF; `overrides` is zero
or more `group=variant-or-none` lines sorted by group bytes with one final LF when non-empty; and
`state-version` is exactly `1` plus LF. Duplicate/unknown group lines, malformed UTF-8, or
extra whitespace are errors, never last-wins input. The ownership ledger and HMAC key use separate
owner-only files `ledger.json` and `hmac.key` in this directory; the directory is `0700`, the files
are `0600`, and `hmac.key` is exactly 32 random bytes created with an OS CSPRNG on first apply.

A theme reference resolves the basename of a tracked `theme/profiles/<name>.dotfile` source. The
theme source schemas and renderer boundary are defined in section 26.4. Effective theme
precedence is explicit CLI theme, facet theme, group-root theme, host theme, profile theme, then
repository `@theme`. A missing final theme is an error for a command that needs theme generation.
Theme commands read theme source contents directly rather than duplicating them in the generated
lock; the available-name inventory and resolved references enter resolution IR.
If a generated theme artifact is a materialized deployment leaf, its final generated bytes follow
the ordinary copy-digest rule.

Only tracked ordinary `.dotfile` files directly under `theme/profiles` contribute names; each
basename MUST be an `IDENT`, and duplicates are errors. Group-root `@theme` contributions use the
scoped-scalar merge in section 11: a descendant replaces an ancestor, equal active sibling values
coalesce, and disagreeing active siblings invalidate the profile. Profile order never picks a
group theme. A facet `@theme` then replaces the merged group theme for that facet.

`.dotfile` version 1 permits exactly one repository default declaration, the optional top-level
`@theme = <theme-reference>` in `config/profiles.dotfile`. It produces one `@defaults` record with
`key = "theme"` and the qualified theme ID as `value`; if absent, `@defaults` is empty. No other
default key is valid in generated lock version 1.

## 15. Per-profile node resolution and checks

After demand folding and fact merging, each active node has a profile-specific shape.

### 15.1 Check adapter

`@check`, when present, selects the adapter. Otherwise form the set of applicable specialized
shapes:

1. `font` resource or resolved `@family` → `font`;
2. resolved `@service` → `service`;
3. resolved entity `@path` → `path`;

If exactly one of `font`, `service`, or `path` applies, select it. If more than one applies, the
node shape is an error. If none applies, select `command`.

Explicit adapters must be compatible with resolved attributes. Adapter results are stored per
profile, not globally.

| Adapter | Required data | Observation |
|---|---|---|
| `command` | effective `@bin` or entity name | executable present on user PATH |
| `package` | effective install coordinate | package-manager query |
| `font` | non-empty `@family` | any family installed; note non-preferred fallback |
| `service` | `@service`, optional `@scope` | enabled/loaded through platform adapter |
| `path` | entity `@path` | machine path exists |
| `none` | none | tracked/emitted, never observed |

Install metadata (`@pkg`, `@installer`, `@description`) may accompany every check shape. Shape
attributes obey this exhaustive compatibility matrix; an attribute in a forbidden column is an
error even when an explicit `@check` would otherwise ignore it.

| Check | Required/allowed shape attributes | Forbidden shape attributes |
|---|---|---|
| `command` | optional `@bin`, optional `@version` | `@family`, `@service`, `@scope`, `@path` |
| `package` | optional `@version`; completed install coordinate | `@bin`, `@family`, `@service`, `@scope`, `@path` |
| `font` | non-empty `@family` | `@bin`, `@service`, `@scope`, `@path`, `@version` |
| `service` | `@service`, optional `@scope` | `@bin`, `@family`, `@path`, `@version` |
| `path` | `@path` | `@bin`, `@family`, `@service`, `@scope`, `@version` |
| `none` | none | `@bin`, `@family`, `@service`, `@scope`, `@path`, `@version` |

Path nodes with `@deploy = "none"` use their own repository-path observation and are not entities.

`@version` is valid only with `command` or `package`. The built-ins-version-1 command strategy
invokes the registered executable directly with adapter-defined `--version` arguments and requires
the wanted string in the bounded first output line. The package strategy searches the
manager-reported version. Other adapters reject `@version`.

Adapters are versioned built-ins. Source cannot supply shell fragments, arbitrary argv, executable
paths for internal helpers, timeouts, or environment variables. Compilation never runs an adapter.

A command name—authored `@bin` or an entity-name default—MUST match
`[A-Za-z0-9][A-Za-z0-9._+-]*`. It cannot contain `/`, be `.` or `..`, or carry surrounding
whitespace. The command adapter resolves only that basename on its sanitized PATH and invokes a
fixed registry argv; it never executes an absolute, relative, or repository path from source.

### 15.2 Install coordinates and emission

For every active demanded node, the compiler resolves:

```text
InstallCoordinate {
    installer   @installer or profile manager's default installer
    package     @pkg or entity name
    version     optional @version
}
```

A resource has no default package name; it is emitted only when `@pkg` resolves. Every completed
coordinate is validated against its installer adapter and stored with field-level provenance.

Package emission performs a second aggregation by `(installer, package)`:

- duplicate coordinates are emitted once while retaining every node/reason;
- required wins if any contributing node is required;
- incompatible non-empty versions for one coordinate are an error;
- one non-empty version combined with any number of unversioned contributions emits that non-empty
  version;
- optional-only coordinates are omitted unless `--optional` is requested;
- output is sorted first by registered installer order, then by unsigned UTF-8 bytes of package
  name.

Thus `wl-copy` and `wl-paste` may both resolve to `wl-clipboard` without producing duplicate install
lines.

### 15.3 Built-in adapter definitions

`.dotfile` version 1 and generated lock version 1 use built-ins version `1`; its adapter subset is
normative data, not a host or tool-version input. Installer order, compatibility, defaults, and
query behavior are:

| Order | Installer | Profile manager(s) | OS | Default | Installed-version query |
|---:|---|---|---|---|---|
| 10 | `brew-formula` | `brew` | Darwin | yes | fixed argv `brew list --versions <package>` |
| 20 | `brew-cask` | `brew` | Darwin | no | fixed argv `brew list --cask --versions <package>` |
| 30 | `pacman` | `pacman` | Linux | yes | fixed argv `pacman -Q <package>` |
| 40 | `aur` | `pacman` | Linux | no | fixed argv `pacman -Q <package>` |
| 50 | `apt` | `apt` | Linux | yes | fixed argv `dpkg-query -W -f=${Version} <package>` |
| 60 | `cargo` | `brew`, `pacman`, `apt` | Darwin, Linux | no | fixed argv `cargo install --list`; exact package record |
| 70 | `uv` | `brew`, `pacman`, `apt` | Darwin, Linux | no | fixed argv `uv tool list`; exact package record |

The default row for the profile manager supplies its installer. Package strings are non-empty,
single-line UTF-8 with no leading/trailing whitespace or NUL. Versions, when present, have the same
constraints. Registry invocations never use a
shell, inherit only a minimal locale-fixed environment plus sanitized PATH, time out after 10
seconds, and read at most 64 KiB of output. `command` uses no process when only presence is checked;
with `@version` it invokes `[bin, "--version"]` and searches the first decoded output line.
`font` uses CoreText on Darwin and Fontconfig's library API on Linux. `service` uses fixed
`launchctl print` domains on Darwin and fixed `systemctl is-enabled` user/system argv on Linux.
`path` uses `lstat`; `none` observes nothing.

`dotfile packages <profile> --emit <installer>` emits canonical RFC 8785 JSON Lines, one coordinate
object per line and one final LF. Every object has `installer`, `mode`, and `package`; `version` is
present only when non-empty. The requested installer is still written into each object so outputs
remain self-describing. Object keys use JCS ordering, coordinates use the aggregation/sort rules in
section 15.2, and no command or shell fragment is emitted.

## 16. Source and facet discovery

Discovery is deterministic and does not walk arbitrary ignored build trees.

1. Parse `config/profiles.dotfile` and resolve group directories.
2. For each directory-bearing group in group-name byte order, read its optional root
   `package.dotfile`.
3. Its package candidates are immediate, ordinary child directories except `overrides`.
   Directory symlinks are not followed.
4. A candidate containing `package.dotfile` declares one facet. An empty file is valid. A candidate
   without it is an `undocumented-package` warning and is not deployed.
5. Nested directories inside a facet are payload, never additional packages.
6. Discover group override variants as specified in section 19.

The logical package name is the immediate directory basename and MUST be representable as `IDENT`.
Same-named facets in different groups remain distinct roots but share a logical package name for
default destinations and deployment overlays.

Deployment inventory is the union of:

```text
tracked index entries under the facet
union untracked working-tree entries not excluded by tracked repository .gitignore files
```

Only `.gitignore` files present in the Git index are inputs. Matching uses Git's documented
`.gitignore` pattern semantics, anchored at each ignore file's directory; the language conformance
fixtures freeze every supported pattern edge case. The compiler MUST NOT consult
`.git/info/exclude`, `core.excludesFile`, global configuration, environment, or an untracked
`.gitignore`. An untracked `.gitignore` encountered under a declared group is an error rather than
payload. Raw hashes of the tracked ignore files enter `@structure`.

Type information is read without following symlinks. A tracked-but-missing entry is an error.
Ignored entries are not payload, which keeps generated trees such as `shared/wezterm/types` out of
the committed plan. An untracked, non-ignored entry may be compiled during authoring, but CI MUST
reject a committed lock whose declared source is absent in a clean checkout.

For a tracked path, the index mode is authoritative: `100644`, `100755`, or `120000`. The current
worktree object MUST exist and its no-follow type MUST agree (regular versus symlink); regular-file
execute-bit drift does not change the index-derived mode. Current worktree bytes still supply
source hashes/digests, so uncommitted content is visible. For an untracked regular file, `lstat`
maps a set owner-execute bit to provisional `100755` and an unset owner-execute bit to `100644`;
other permission bits do not enter IR. An untracked symlink maps to `120000`. CI's clean-checkout
rule ensures a committed lock ultimately uses index modes.

Every selected repository path MUST decode as strict UTF-8 NFC, use `/`, and contain no control,
empty, `.` or `..` component. Byte-only Git pathnames are unsupported and rejected with their
escaped byte spelling.

Exactly these source object types are supported:

- ordinary regular file (`100644` or `100755` in Git);
- symbolic link (`120000`) satisfying section 17.2.

Gitlinks/submodules, sockets, devices, FIFOs, and other special objects are errors when they occur
in payload. Hardlinks are ordinary files. Empty directories are not represented by Git and are not
deployed.

The following are metadata, not payload:

- the facet-root `package.dotfile`;
- a variant-facet-root `package.dotfile`;
- any tracked `.gitignore` used for inventory selection;
- `package.lock.dotfile`;
- reserved invalid marker files `.nolink`, `.secret`, and `.system`.

Encountering one of those marker files is an error with an automated fix suggestion.

## 17. Leaf enumeration and source safety

### 17.1 Leaves only

Regular directories are traversal structure. The compiler emits one candidate deployment row per
eligible leaf. Compiler-synthesized destination directory links and directory rows are invalid in
generated lock version 1.

Leaf-only deployment permits co-located `package.dotfile` metadata, exact collision detection, and
lock freshness. Destination directories are real directories containing leaf links or files;
directory-unfolding target entries are not part of the language.

### 17.2 Source symlinks

An authored source symlink is one leaf and is never traversed, even when its target object is an
in-repository directory. That single symlink may be deployed as a link; the compiler never expands
it and never turns an ordinary source directory into a destination directory link. Its raw target
MUST:

- be UTF-8 and relative;
- contain no empty, `.` or `..` component after lexical combination with its parent;
- resolve lexically inside the repository;
- resolve to an existing repository object at compile time;
- not pass through another escaping or broken symlink.

Absolute, broken, escaping, or platform “magic” links are errors. The raw link target and leaf type
enter the structure fingerprint. `copy`, `sops`, and `template` rows require ordinary regular-file
sources; only `link` may deploy a validated source symlink.

For every `link` candidate, binding resolves the physical repository root once by directory
descriptor and forms an absolute target to that candidate's `physical_source`. The destination
symlink stores that absolute target byte-for-byte. It never stores a path relative to the
destination. When the source leaf is itself an authored symlink, the destination links to that
symlink object; it does not copy/rebase the authored symlink's raw target. Thus the resulting link
may be a two-link chain. “Exactly desired” means the
destination is a symlink whose raw target is this bound absolute path, not merely one that happens
to resolve to the same inode.

### 17.3 Plain, encrypted, and template source names

Render behavior is explicit in IR and derived from these filename forms:

| Source basename | Effective render | Destination basename |
|---|---|---|
| `name` | `plain` | `name` |
| `name.enc` | `sops` binary | `name` |
| `name.enc.<tail>` | `sops` structured | `name.<tail>` |
| `name.tmpl` | `template` | `name` |

A basename matching more than one transform pattern is an error. In `name.enc.<tail>`, `<tail>` is
the entire non-empty remainder and may itself contain dots; the transform removes exactly the
first `.enc` immediately before it, so `archive.enc.tar.gz` becomes `archive.tar.gz`. Render is
derived only from these filename forms in `.dotfile` version 1. It cannot be authored, suppressed, or
applied to a nonstandard basename. `logical_source` retains the pre-transform basename;
`output_source` is the same logical path with this one basename transform applied.

`sops` and `template` rows under `link` or `copy` are private materialized copies, regardless of
the enclosing facet's plain-leaf action or `@sensitivity`; an effective `none` excludes them before
render derivation. `@sensitivity` configures plain leaves only; no more-specific
attribute can declassify a transformed leaf. A facet with `@sensitivity = "private"` MUST contain
only transformed leaves; an ordinary plaintext leaf is a compile error. Ordinary public link
facets may contain individual encrypted/template leaves, which materialize while their siblings
link.

Templates are UTF-8 and may reference only the separately specified vault variable grammar. That
template grammar is not a general `.dotfile` expression language.

## 18. Deployment claim expansion

### 18.1 Exact mappings

A mapping is an exact pair:

```text
Mapping(source_prefix, destination_root)
```

A normal link facet has an implicit mapping:

```text
source_prefix    facet root
destination_root ~/.config/<logical-package-name>
```

An explicit facet `@destination` replaces that root exactly. It never appends the package name.
A path node with `@destination` adds a longer source-prefix mapping.

For each pre-transform `logical_source` path `L`, select the mapping whose source prefix `P` is the
longest matching sequence of complete path components. Let `R` be `L` relative to `P`. If `R` is
non-empty, apply the section 17.3 basename transform to its final component, producing `R'`:

```text
destination(L) = destination_root                 when R is empty
destination(L) = destination_root + "/" + R'      otherwise
```

Thus an exact leaf `@destination` (`R` empty) is already the final filename and may intentionally
rename a transformed source; the compiler never strips a suffix from the authored destination.
Parent mappings append the transformed remainder. Mapping selection and variant replacement always
use pre-transform `logical_source`; destination collision checks use the final destination.

Matching is component-based: `foo` does not match `foobar`. Equal-length mappings for one leaf are
an error unless their normalized destinations and properties are identical.

Example for `shared/starship/starship.toml`:

```dotfile
# no explicit destination
# -> ~/.config/starship/starship.toml

@destination = "~/.config"
# -> ~/.config/starship.toml
```

The second form maps the leaf directly to `~/.config/starship.toml`; no directory row at
`~/.config/starship` is emitted.

### 18.2 Coverage by action

- A `link` facet receives the implicit default mapping unless an explicit facet destination
  replaces it.
- A `copy` facet with user privilege MUST have explicit facet or path mappings covering every
  payload leaf.
- A system-privileged facet receives no implicit mapping; every leaf MUST be covered explicitly.
- A `none` subtree emits no rows.

This system package is exact:

```dotfile
@deploy = "copy"
@privilege = "system"
@owner = "root"
@group = "root"

./etc {
    @destination = "/etc"
    @mode = "0644"
}
```

`etc/systemd/network/10-macie.link` maps to `/etc/systemd/network/10-macie.link`. The compiler never
silently chooses or strips an `etc` child.

### 18.3 Effective leaf record

Before collisions, every leaf has:

```text
DeploymentCandidate {
    facet               qualified facet or variant facet
    declaration_group
    variant             optional group variant
    physical_source     repository-relative source path
    logical_source      source path after variant-prefix stripping
    output_source       logical source after filename transformation
    destination         symbolic normalized destination
    action              link | copy
    render              plain | sops | template
    privilege           user | system
    sensitivity         public | private
    mode                optional/required by action
    owner, owner_group  required for system copy
    source_type         regular | symlink
    source_digest       required for materialized source identity
    provenance          mapping and attribute spans
}
```

For ordinary links, `source_digest` is omitted and source content is deliberately not hashed. For
a plain copy, `source_digest` is the plaintext repository-file SHA-256. For SOPS it is the
ciphertext SHA-256; no plaintext digest enters the lock. For templates, `source_digest` is the
public template SHA-256 and separate fields record `vault_source` plus `vault_digest`, the SHA-256
of the exact raw ciphertext bytes of the configured encrypted variable store (currently
`vars.enc.yaml`). No decrypted or rendered value enters the lock.

## 19. Override variants

Overrides are group-scoped machine variants.

```text
<group-directory>/overrides/<variant>/<package>/...
```

Rules:

1. `<variant>` and `<package>` are `IDENT`s and ordinary directories. A variant name MUST NOT be
   `base` or `none`; those two strings are reserved respectively for the base-row lock tag and the
   explicit no-variant machine-state selection.
2. Every variant package MUST contain a `package.dotfile`; it may be empty when the variant relies
   entirely on inherited defaults.
3. A variant package file permits bindings, facet deployment attributes, and path nodes only. It
   cannot add demands or facts; machine state never changes the semantic dependency graph.
4. Its physical prefix is
   `<group>/overrides/<variant>/<package>`; its logical prefix is `<group>/<package>`.
   `overrides/<variant>` is stripped exactly once before destination mapping.
5. When a base same-named facet exists in the group, the variant facet inherits its facet-level
   deployment properties and mappings, then applies explicit variant replacements. Otherwise it
   inherits the language defaults.
6. The lock contains every variant's rows. Machine state selects at most one variant name per
   active group, or the explicit selection `none`.
7. A selected variant leaf replaces the base leaf with the same logical source path within that
   group. Replacement removes the base candidate completely, even when the variant remaps that
   logical leaf to a different destination. Base leaves at other logical paths remain; a variant
   leaf with a new logical path is added.
8. There is no whiteout/removal marker in `.dotfile` version 1. A future removal form requires a
   new `.dotfile` version.
9. An unknown, inactive-group, or path-interpolated selection is an error. If a group has variants
   and no selection is saved or supplied, no variant is active. Read-only status MUST warn and MAY
   show a base-only preview clearly marked non-applicable. A destination-mutating command MUST
   refuse until every active group that declares variants has an explicit invocation or saved
   selection of either one variant or `none`; it never treats missing state as `none`.

The state selection scope is one variant label per group—not per facet. The lock retains group,
variant, physical path, logical path, and base/variant precedence for every row.

## 20. Collision and permission rules

### 20.1 Compile-time destination equality

Destinations are first validated in symbolic form and keyed by anchor plus path components. The
compiler validates each declared profile against the Cartesian product of selections—exactly one
of `none` or a variant for every active group that declares variants—so a collision visible only
when two groups' variants are selected together is still rejected. For each such bound plan:

1. Candidates deduplicate operationally only when this complete operational tuple is equal:
   `(physical_source, logical_source, output_source, destination, action, render, privilege,
   sensitivity, mode-or-empty, owner-or-empty, owner_group-or-empty, source_type,
   source_digest-or-empty, vault_source-or-empty, vault_digest-or-empty)`. Facet/group/variant and
   every provenance origin are retained on the one operational row. Different physical sources
   never deduplicate merely because their bytes match.
2. A selected variant outranks its base candidate as defined above.
3. Within one group, different sources claiming the same destination are an error after variant
   handling.
4. Across groups, different sources with the same destination and same action may overlay; the
   later group in the profile's active order wins. Shadowed rows remain in the lock and `why` data.
5. Different deployment actions at one destination are always an error.
6. A destination that is an ancestor or descendant of another leaf destination is a destination
   prefix collision and is always an error, across all groups and actions. Profile order never
   resolves a file-at-ancestor collision. This is distinct from overlapping source mappings,
   which use the longest-component-prefix rule in section 18.1.

For Darwin profiles, the compiler additionally rejects destinations equal under Unicode NFD plus
default case folding. Linux profiles use exact UTF-8 spelling. The binder MUST recheck collision
equivalence against the actual destination volume because APFS may be case-sensitive or
case-insensitive.

### 20.2 Modes and ownership

Modes are quoted four-digit octal strings.

| Row | Default/requirement |
|---|---|
| user link | no mode; mode attributes invalid |
| user public plain copy | `0644` unless explicit |
| SOPS/template copy, user or system | locked to `0600`, or `0700` when the source executable bit is set |
| system plain copy | explicit mode, owner, and group required |
| newly created public user directories | `0755` |
| newly created private user directories | `0700` |
| newly created system directories | derived from the approved system row; default `0755` only beneath an existing top-level directory |

Plain executable material requires explicit `0755` or owner-only `0700`; transformed executable
material is locked to `0700`. A parent `@mode = "0644"` therefore governs its plain descendants but
cannot weaken a transformed descendant: that row resolves to `0600` or `0700`. System rows do not
infer arbitrary plain-file modes from a checkout because Git only preserves the executable bit.
The lock serializes the resolved mode and ownership.

Bound plans and the machine ledger canonicalize ownership independently of account spelling.
`owner` is `uid:<n>` and `owner_group` is `gid:<n>`, where `<n>` is unsigned decimal with no
leading zero except `0`. A user row uses the invoking account's resolved numeric UID and primary
GID. A system row resolves its authored `@owner` and `@group` through the target account database
before preflight; an absent, ambiguous, or changed mapping is an error. Desired-state checks
compare the numeric IDs. These canonical strings are also the `O` and `G` values used in every
directory prerequisite ID and private verifier.

Parent directories are planned as first-class prerequisites before any leaf operation. For each
missing parent, collect every winning descendant row that requires it:

- mixing user and system privilege at that directory is an error;
- a user directory is owned by the invoking account and resolves to `0700` if any descendant is
  private, otherwise `0755`;
- system descendants MUST agree on owner and owner-group; mixing public and private descendants in
  one newly created system directory is an error, otherwise mode is `0700` for private or `0755`
  for public;
- an existing parent is never chmod/chowned implicitly and MUST already be a real, non-symlink
  directory traversable by the intended identity;
- a system plan may not create a direct child of `/`. It may create deeper parents only below a
  pre-existing top-level directory such as `/etc` or `/var`.

The resulting `(path, privilege, owner, owner_group, mode)` prerequisite set is deduplicated by
exact equality; any other value conflict blocks the whole plan.

## 21. Safe application semantics

Compilation authorizes nothing. A repository can validly declare a dangerous destination; hashes
establish identity, not publisher trust. Checking out a repository is never permission to mutate a
machine.

Every deployment consumer that creates, replaces, or prunes destination state MUST:

1. acquire an exclusive machine-state lock;
2. recompile unprivileged and byte-compare the canonical lock, not merely trust header hashes;
3. bind an explicit/unambiguous profile and variant set;
4. resolve the invoking user's home from OS account identity before elevation, not inherited
   `$HOME`;
5. validate OS/architecture, reject destinations inside the physical repository, and recheck
   actual-volume destination equality;
6. compute the complete operation set and all conflicts before writing;
7. require separate, exact confirmation for privileged rows unless an explicit noninteractive flag
   was supplied;
8. execute without running repository content;
9. journal completed operations and report rollback status on failure.

Destination state is classified as:

| State | Action |
|---|---|
| absent | create |
| exactly desired | no-op |
| recorded as tool-managed and still matches recorded identity | replace or prune as planned |
| anything else | hard foreign-destination conflict |

The owner-only `ledger.json` is canonical JCS with `"ledger-version": "1"` and an `entries` array
sorted by bound destination bytes. Every entry stores destination, object kind, resolved mode/owner metadata,
and the platform's guarded `object_token` (volume identity, file identity, and generation/change
token). A leaf entry additionally stores `candidate_id`, the lowercase `sha256:` digest of that
candidate's canonical JSON record. A link stores its raw absolute target; a public copy stores its
byte SHA-256; a private copy stores only the keyed verifier defined below.

A synthesized directory entry has kind `directory`, omits `candidate_id`, and instead stores
`prerequisite_id`. Its value is the lowercase `sha256:` digest of RFC 8785 JCS for exactly this
object, with every value a string resolved by section 20.2:

```text
{"destination": D, "kind": "directory", "mode": M, "owner": O, "owner_group": G, "privilege": P}
```

`D`, `M`, `O`, `G`, and `P` above denote JSON string values, not literal identifier tokens.
Whitespace is illustrative; the hashed bytes are JCS. `D` is the bound absolute destination and
the other values are the exact resolved prerequisite-tuple values. This directory object is a
ledger identity only; it does not create a forbidden deployment candidate or lock record. If a
filesystem cannot provide a stable token suitable for guarded replacement, the entry remains
observable but is never automatically replaced or pruned.

Pruning is limited to the machine state ledger. It never scans for arbitrary links into the
repository and never deletes an unrecorded path. A ledger-owned object modified since application
is a conflict, not silently removed. Adoption/force requires a separately specified command that
backs up and records the prior object; it is not implicit in `link`.

Writes use descriptor-relative, no-follow operations where supported. Every parent component is
validated immediately before the final operation. The repository root is held by descriptor.
Immediately before use, every link source is `lstat`-verified against locked type/mode/raw-target
identity; every materialized source and vault input is opened no-follow, `fstat`-verified, and
digested from that same descriptor. Rendering/decryption consumes only those verified descriptors.

Materialized files are written to a unique same-directory temporary, mode/ownership is set through
the descriptor, and the descriptor is flushed before installation. Creation uses a kernel
no-replace operation, so a newly appearing leaf yields a conflict.

For this specification, `guarded_replace(parent, name, expected_token, new_object)` and
`guarded_prune(parent, name, expected_token)` are semantic filesystem primitives. Each MUST be one
linearizable kernel/filesystem namespace operation: at its linearization point it succeeds only if
`name`, beneath the already opened no-follow `parent`, still denotes the exact volume, file, and
generation/change identity encoded by `expected_token`. On mismatch it returns a destination-race
conflict and makes no namespace mutation. On success it atomically substitutes `new_object` or
removes that one entry. A token qualifies only when its platform adapter guarantees that any
replacement or content/metadata change relevant to the ledger's desired-state test changes the
token. A userspace check followed by unconditional rename/unlink, or an advisory lock that only
cooperating processes honor, does not implement either primitive.

Replacement or prune of a ledger-owned object MUST use that contract. If the platform or
destination filesystem cannot provide it, the tool refuses automatic replacement/prune and
requires a separately confirmed backup/adopt workflow. It never falls back to a racy overwrite.
Multi-file application is not claimed atomic; completed guarded operations are journaled and
rollback is best-effort.

The main process never runs as root. A minimal privileged helper accepts only an already approved
system-copy plan plus opened/verified data descriptors. It does no parsing, network access,
decryption, shell execution, or arbitrary command execution, and independently performs the same
guarded destination CAS plus digest, mode, owner, group, and foreign-leaf checks.

Secrets and rendered private data MUST NOT appear in the lock, argv, environment, temporary names,
logs, diffs, diagnostics, or state ledger. The ledger MAY retain only an HMAC-SHA-256 verifier made
with a random per-machine 256-bit key stored separately at owner-only permissions. The HMAC input
is the ASCII bytes `dotfile-private-ledger-v1`, one zero byte, then these six fields in order:
`destination`, `candidate_id`, exact rendered bytes, `mode`, `owner`, and `owner_group`. Each field
is encoded as an unsigned 64-bit big-endian byte length followed by that many bytes. Text fields
use their exact canonical ledger UTF-8 bytes; an absent optional text field is the zero-length byte
string. The ledger stores the result as `hmac-sha256:` followed by 64 lowercase hexadecimal digits.
It never stores an unkeyed plaintext digest. Status recomputes this exact HMAC over the destination
to detect modification without revealing content. Loss of the key makes private leaves
unverifiable and therefore conflicts until explicit re-adoption. Decryption/rendering occurs
unprivileged, passes bytes by pipe or descriptor, uses private temporary storage only when
unavoidable, and minimizes plaintext lifetime. Secret diffs report state and ciphertext/provider
identity, never plaintext.

`--dry-run` performs the same compilation, binding, collision, ownership, and permission checks but
makes no mutation.

## 22. The generated lock file

`package.lock.dotfile` is committed, generated, canonical, and never hand-edited. It is a lossless
serialization of resolved semantic IR—not a source round trip and not machine observations. Its
format version is independent of the `.dotfile` source version as explained in section 1.

### 22.1 Required sections

Generated lock version 1 has this exact section order:

1. `@lock`
2. `@sources`
3. `@groups`
4. `@profiles`
5. `@facets`
6. `@nodes`
7. `@facts`
8. `@occurrences`
9. `@paths`
10. `@mappings`
11. `@effective`
12. `@deployments`
13. `@themes`
14. `@hosts`
15. `@defaults`

Every section is emitted, including an empty one. Optional record fields are omitted; there is no
lock `null` value.

### 22.2 Header and source records

```dotfile
# Generated by `dotfile lock`. Do not edit.
@lock {
    @dotfile-version = "1"
    @lock-version = "1"
    @builtins-version = "1"
    @ir = "sha256:..."
    @structure = "sha256:..."
}

@sources {
    source { path = "config/profiles.dotfile", domain = "profiles", hash = "sha256:..." }
}
```

The lock omits timestamps, hostnames, absolute repository paths, random IDs, and tool version, so
conforming implementations produce identical bytes. Source hashes cover the exact raw bytes of
every resolution-domain source file. CRLF-to-LF parsing does not change the source hash.

`source.domain` has exactly five values: `"profiles"` for `config/profiles.dotfile`, `"hosts"` for
`config/hosts.dotfile`, `"group"` for a group-root `package.dotfile`, `"facet"` for a base facet
file, and `"variant"` for an override-variant facet file. A missing optional group-root file has no
source record; every existing semantic file has exactly one.

The structure digest covers canonical records for:

- Git-selected payload relative path, object type, and executable bit;
- source-symlink raw target;
- copied-source content digest;
- group directory map and facet coverage;
- override variant and variant-facet inventories;
- available theme names, but not theme-definition contents;
- tracked `.gitignore` paths and raw-byte digests;
- the fixed encrypted template-variable store's path and ciphertext digest;
- no ambient or unnamed external input.

Precisely, `@structure` is SHA-256 over RFC 8785 JCS bytes for an object with six arrays:
`files`, `groups`, `facets`, `themes`, `ignores`, and `vault_inputs`. `files` records are ordered by
repository-relative path
and contain `path` plus Git-style `mode` (`100644`, `100755`, or `120000`), then `raw_target` for a
symlink or `content_digest` when that regular file feeds at least one materialized candidate.
`groups` contains `(id, directory?)` ordered by `id`; `facets` contains
`(id, directory, variant)` ordered by `(id, variant)`; `themes` contains theme-name strings in byte
order; `ignores` contains `(path, raw_digest)` in path order; `vault_inputs` is empty or contains
the single `(path, ciphertext_digest)` record for `vars.enc.yaml`.
Object keys are exactly those names, and record keys are exactly the fields just listed.

Ordinary linked file contents are excluded. Adding, removing, renaming, or changing the type of a
link leaf changes the structure digest. Changing a plain linked file's content does not.

### 22.3 Typed record schema

All record identity fields use qualified strings. The minimum fields are:

| Section / record | Required fields | Optional fields |
|---|---|---|
| `@sources / source` | `path`, `domain`, `hash` | none |
| `@groups / group` | `id`, `name`, `ancestors` | `parent`, `directory`, `os`, `arch`, `description` |
| `@profiles / profile` | `id`, `groups`, `manager`, `installer`, `os` | `arch`, `theme`, `description` |
| `@facets / facet` | `id`, `group`, `package`, `directory`, `variant`, `source_span` | `description`, `theme`, `destination`, `deploy`, `privilege`, `sensitivity`, `mode`, `owner`, `owner_group` |
| `@nodes / node` | `id`, `node_kind` | `resource_kind`, `resource_key` |
| `@facts / fact` | `target`, `attribute`, `scope`, `value`, `source_span` | none |
| `@occurrences / occurrence` | `id`, `target`, `root`, `group`, `local_mode`, `effective_mode`, `source_span`, `reasons` | `parent` |
| `@paths / assertion` | `id`, `facet`, `path`, `demand_mode`, `expect`, `source_span` | none |
| `@mappings / mapping` | `facet`, `source_prefix`, `deploy`, `privilege`, `sensitivity`, `origin`, `source_span` | `destination`, `mode`, `owner`, `owner_group` |
| `@effective / resolution` | `profile`, `target`, `demand_mode`, `check`, `provenance` | `bin`, `family`, `service`, `scope`, `path`, `installer`, `package`, `version` |
| `@deployments / candidate` | `facet`, `declaration_group`, `variant`, `physical_source`, `logical_source`, `output_source`, `destination`, `action`, `render`, `privilege`, `sensitivity`, `source_type`, `provenance` | `mode`, `owner`, `owner_group`, `source_digest`, `vault_source`, `vault_digest` |
| `@themes / theme` | `id`, `name`, `path` | none |
| `@themes / contribution` | `group`, `theme`, `source_span` | none |
| `@themes / theme_resolution` | `profile`, `provenance` | `group_theme`, `profile_theme`, `default_theme` |
| `@hosts / host` | `id`, `name`, `hostnames`, `role`, `profile` | `theme` |
| `@hosts / fact` | `host`, `key`, `value` | none |
| `@defaults / default` | `key`, `value`, `source_span` | none |

`variant` on an ordinary facet is the data string `"base"`; a variant facet stores its variant
name. Arbitrary host facts are never dropped. Every scoped contribution is retained even when it
is shadowed. Every duplicate demand occurrence is retained. Every deployment candidate and its
mapping/attribute provenance is retained even when it deduplicates or loses cross-group
precedence. `@mappings` retains every effective facet-root or path-prefix claim, including an empty
implicit facet mapping or an authored mapping that matches leaves but loses every longest-prefix
selection; `origin` is `"implicit"` or `"authored"`.
`source_digest` is omitted for an ordinary link and required for every materialized candidate;
template candidates additionally require `vault_source` and `vault_digest`.

For `group.ancestors`, `shared` has `[]`; every other group lists `group:shared` first and then its
declared ancestors from outermost root to immediate parent, without duplicates. `profile.groups`
stores the complete active order from section 13, including `group:shared`.

`@themes/theme` serializes the complete tracked name inventory. Every group-root declaration is a
`contribution`, including one shadowed during merging. Exactly one `theme_resolution` is emitted
per profile: `group_theme` is the merged group-root result, while `profile_theme` and
`default_theme` preserve their lower-precedence candidates. The binder then applies the precedence
in section 14 after adding host, facet, and CLI inputs; it never has to rediscover a theme fact.

The mandatory `theme_resolution.provenance` list has exact membership. A present `group_theme`
contributes the source origins of precisely the maximal group declarations whose equal values form
the merged result, with output field `group_theme`; shadowed contributions are not included. A
present `profile_theme` contributes its declaration source origin with field `profile_theme` and
the built-in origin with that field and rule `theme/profile`. A present `default_theme` contributes
its declaration source origin with field `default_theme` and the built-in origin with that field
and rule `theme/repository-default`. An absent field contributes nothing. The origins are
deduplicated and sorted by the general rule below; the list is therefore `[]` when all three fields
are absent.

For each `@effective/resolution`, conditional fields are emitted by this exact matrix:

- `bin` is always present for `check = "command"`;
- `family` is always present for `font`;
- `service` and resolved `scope` (including default `"user"`) are always present for `service`;
- `path` is always present for `path`;
- none of those shape fields is present for another check;
- `installer` and `package` are either both present for a completed install coordinate or both
  absent for a non-emitted resource; an entity always has a completed coordinate;
- `version` is present exactly when a non-empty version resolves;
- `provenance` is always present, including built-in defaults.

Every `provenance` value is an ordered list of canonical origin strings. A source origin is
`s:<field_byte_length>:<field>:<source_span>`; a built-in origin is
`b:<field_byte_length>:<field>:<rule_byte_length>:<rule>`. Lengths count UTF-8 bytes and use
canonical unsigned decimal. `field` is the exact output field receiving the value (for example
`installer`, `destination`, or `mode`); `rule` is a built-ins-version-1 ASCII rule ID such as
`manager/brew/default-installer`. Origins sort by `(field bytes, source-before-built-in, span-or-rule
bytes)` and exact duplicates collapse. A merged/deduplicated row retains the union, so field-level
origin is never inferred from list position.

Provenance covers merged facts, inferred/default checks and coordinates, mapping selection, and
deployment properties; identity/path fields already name their own origin and do not need a
provenance item. Built-ins version 1 permits exactly these built-in rule IDs:

```text
check/infer/font              check/infer/service
check/infer/path              check/infer/command
check/scope/user              node/bin/entity-name
install/package/entity-name  manager/brew/default-installer
manager/pacman/default-installer
manager/apt/default-installer
deploy/action/link            deploy/privilege/user
deploy/sensitivity/public     deploy/destination/config-package
deploy/mode/user-public       deploy/render/plain
deploy/render/sops-binary     deploy/render/sops-structured
deploy/render/template        deploy/render/force-copy
deploy/render/private         deploy/render/mode-0600
deploy/render/mode-0700       deploy/mapping/longest-prefix
theme/profile                 theme/repository-default
```

An implementation cannot mint another built-in provenance rule without new built-ins and
generated-lock versions.

`source_span` has this exact lock-string encoding:

```text
<path_byte_length>:<path>:<start_byte>:<end_byte>:<start_line>:<start_column>:<end_line>:<end_column>
```

The length and coordinates are canonical unsigned decimal with no leading zero except `0` itself;
the length counts UTF-8 path bytes and lets a path contain `:` unambiguously. Byte offsets are
zero-based and half-open. Lines and Unicode-scalar columns are one-based, with the end position
exclusive. A facet's file-level anchor is the zero-length span at byte 0, line 1, column 1. The
canonical JSON representation instead uses an object with exactly `path`, `start_byte`,
`end_byte`, `start_line`, `start_column`, `end_line`, and `end_column`.

Semantic span anchors exclude leading/trailing trivia, attached comments, separators, and commas:

- a named occurrence spans its optional `?` (when present) through the final byte of its identity
  word, excluding its value or block;
- a resource occurrence spans its optional `?` through the final byte of `@kind`; the separately
  resolved qualified target (and therefore `@key`) is already an ID-hash field;
- a fact/default spans its complete attribute or assignment-sugar entry from first sigil/name
  through the value;
- an authored mapping or assertion spans the path-entry header from optional `?` through the final
  path byte; its individual property provenance uses each complete attribute span;
- an implicit facet mapping and the facet record use the file-level anchor above;
- a group theme contribution, a profile theme candidate, and the repository default theme each use
  the complete span of their own `@theme` attribute.

These anchors, not whole nested blocks, feed IDs and sort keys. Two semantic entries cannot share
the same target and anchor; if schema recovery encounters that impossible duplicate it is an error
rather than an ID tiebreak.

### 22.4 Canonical ordering and field encoding

- Sections use the fixed order above.
- Record types within a section use the order shown below.
- Records sort by the exact tuple key below. Tuple elements compare as ascending unsigned UTF-8
  bytes; `source_span` compares by `(path, start_byte, end_byte)` rather than its display string.
- Fields use the exact order below; `?` marks a conditional field that is omitted when absent.
  Unknown fields, duplicate fields, and out-of-order fields are schema errors.
- All lock scalar values are quoted strings, including qualified IDs and enum values. Lists contain
  quoted strings.
- Defaults are emitted as resolved fields when the generated lock format requires them; consumers never
  recreate an omitted semantic default.
- Each `source`, `group`, `profile`, `facet`, `node`, semantic `fact`, `occurrence`, `assertion`,
  `mapping`, `resolution`, `candidate`, `theme`, `contribution`, `theme_resolution`, `host`, host
  `fact`, and `default` record occupies exactly one physical line, irrespective of the authored-file
  100-column rule.
- The file uses LF and exactly one final newline.

The complete textual layout is also fixed:

1. The first line is exactly ``# Generated by `dotfile lock`. Do not edit.`` followed by LF; it is
   the only comment.
2. `@lock` is a multiline block exactly as in section 22.2: opener at column 1, five attribute
   lines indented four spaces with no comma, closer at column 1.
3. Every following section is separated from the previous block by exactly one empty line. An
   empty section is one line, for example `@paths {}`. A non-empty section has its opener at column
   1, each record indented four spaces, and its closer at column 1.
4. A record is `type { `, then fields in the table order separated by comma plus one space, then
   ` }`. There is one space around `=`, no trailing comma, and no interior newline.
5. A list is `[]` when empty; otherwise it is `["a", "b"]` with comma plus one space. Nested lists,
   when a typed value requires one, use the same rule.
6. Strings use the canonical encoder in section 6.4. There is no alignment padding, trailing
   whitespace, BOM, or alternate empty-block spelling.

These rules, the field table, and the exact section order determine one byte sequence. The banner
is mandatory but is excluded from the canonical JSON IR and both digests.

| Record | Exact field order | Sort tuple |
|---|---|---|
| `@lock` | `dotfile-version`, `lock-version`, `builtins-version`, `ir`, `structure` | singleton |
| `source` | `path`, `domain`, `hash` | `(path)` |
| `group` | `id`, `name`, `ancestors`, `parent?`, `directory?`, `os?`, `arch?`, `description?` | `(id)` |
| `profile` | `id`, `groups`, `manager`, `installer`, `os`, `arch?`, `theme?`, `description?` | `(id)` |
| `facet` | `id`, `group`, `package`, `directory`, `variant`, `source_span`, `description?`, `theme?`, `destination?`, `deploy?`, `privilege?`, `sensitivity?`, `mode?`, `owner?`, `owner_group?` | `(id, variant)` |
| `node` | `id`, `node_kind`, `resource_kind?`, `resource_key?` | `(id)` |
| `fact` | `target`, `attribute`, `scope`, `value`, `source_span` | `(target, attribute, scope, source_span)` |
| `occurrence` | `id`, `target`, `root`, `group`, `parent?`, `local_mode`, `effective_mode`, `source_span`, `reasons` | `(source_span, target, id)` |
| `assertion` | `id`, `facet`, `path`, `demand_mode`, `expect`, `source_span` | `(facet, path, source_span, id)` |
| `mapping` | `facet`, `source_prefix`, `deploy`, `privilege`, `sensitivity`, `origin`, `destination?`, `mode?`, `owner?`, `owner_group?`, `source_span` | `(facet, source_prefix, source_span)` |
| `resolution` | `profile`, `target`, `demand_mode`, `check`, `bin?`, `family?`, `service?`, `scope?`, `path?`, `installer?`, `package?`, `version?`, `provenance` | `(profile, target)` |
| `candidate` | `facet`, `declaration_group`, `variant`, `physical_source`, `logical_source`, `output_source`, `destination`, `action`, `render`, `privilege`, `sensitivity`, `mode?`, `owner?`, `owner_group?`, `source_type`, `source_digest?`, `vault_source?`, `vault_digest?`, `provenance` | `(declaration_group, variant, destination, logical_source, output_source, physical_source, facet)` |
| `theme` | `id`, `name`, `path` | `(id)` |
| `contribution` | `group`, `theme`, `source_span` | `(group, source_span, theme)` |
| `theme_resolution` | `profile`, `group_theme?`, `profile_theme?`, `default_theme?`, `provenance` | `(profile)` |
| `host` | `id`, `name`, `hostnames`, `role`, `profile`, `theme?` | `(id)` |
| host `fact` | `host`, `key`, `value` | `(host, key)` |
| `default` | `key`, `value`, `source_span` | `(key)` |

Within `@themes`, record-type order is `theme`, `contribution`, `theme_resolution`; within
`@hosts`, all `host` records precede all host `fact` records. No other section mixes record types.
A record's conditionally required fields are fixed by its shape—for example, a resource node
has `resource_kind` and `resource_key`, a materialized candidate has `source_digest`, a system
candidate has `owner` and `owner_group`, and a template candidate has both vault fields.

The canonical JSON mapping represents `lock` as one object and every other section as an array of
record objects in the canonical record order. Its value universe is objects, arrays, strings, and
nonnegative integers used only for structured span offsets/coordinates; it has no JSON null,
Boolean, or floating-point value. It is serialized with RFC 8785 JCS. The `@ir` digest is SHA-256
over the UTF-8 JCS bytes of the complete typed IR with only `lock.ir` omitted. Hash text is
`sha256:` plus 64 lowercase hexadecimal digits.

Stable occurrence IDs are `occ:` plus lowercase SHA-256 hex over this exact byte sequence:

```text
UTF8("dotfile-occurrence-v1\0")
|| U64BE(len(UTF8(source_path))) || UTF8(source_path)
|| U64BE(start_byte) || U64BE(end_byte)
|| U64BE(len(UTF8(qualified_target))) || UTF8(qualified_target)
```

`U64BE` is one unsigned 64-bit big-endian integer and `||` is byte concatenation. Files or spans
too large for `U64BE` are errors. Implementations MUST NOT use delimiter-only concatenation or
locale-sensitive sorting.

Assertion IDs use `assert:` plus lowercase SHA-256 hex over
`UTF8("dotfile-assertion-v1\0") || U64BE(len(facet)) || UTF8(facet) ||
U64BE(len(path)) || UTF8(path)`, where lengths count UTF-8 bytes. Facet-local duplicate-path
rejection makes this identity unique.

### 22.5 Freshness and tamper detection

Lock freshness is not a source validation rule:

- `dotfile lock` parses current sources, validates, and writes a new lock whether or not the old
  lock is stale;
- `dotfile lock --check` succeeds only when canonical current output matches the committed file;
- read-only semantic queries MAY use a stale lock but MUST display that state;
- `check` reports staleness as a failing consumer precondition;
- destination-applying commands MUST independently recompile and byte-compare canonical output.

Header hashes alone are insufficient: an attacker could edit a destination row while retaining
old header values. Recompilation and canonical comparison are mandatory before mutation.

This precondition applies to `link` and `system install`, not to compiler/source editors.
`dotfile lock` is specifically allowed to replace a stale lock after successful compilation;
`fmt`, `add`, and `remove` operate on source/CST state and do not apply destination rows. Their own
atomic-write and source-validation rules are separate from section 21.

Machine observations are excluded: selected profile, selected variants, installed versions,
resolved absolute home, destination state, vault plaintext, and last apply result live only in
machine state.

## 23. Canonical source formatting

`dotfile fmt` is total over parse-valid language files and idempotent. It uses the file path to
select a domain schema for ordering, but it does not require successful name resolution or
deployment planning.

General rules:

- four-space indentation;
- one space around `=`;
- no alignment columns;
- target 100 Unicode scalar columns; structural lines wrap before that limit, but one
  unsplittable string/path token or trailing comment may make its own line longer and is never
  rewritten or split;
- canonical `${binding}` interpolation rather than adjacent string atoms;
- no trailing comma in a one-line list/block; a multiline list/block uses one entry/value per line
  and a trailing comma;
- an empty block is `name {}`;
- one final newline for a non-empty file;
- runs of blank lines collapse to one; none immediately after `{` or before `}`.

Within a semantic block:

1. `@let` prologue in source order;
2. identity attributes (`@key`);
3. remaining attributes in the domain's published order;
4. path nodes by decoded path bytes;
5. resource blocks by kind and syntactic key;
6. `@extend` blocks by qualified target;
7. demands by target, required before optional when otherwise equal.

Sorts are stable, so duplicates and invalid/keyless resource blocks retain source order and can
still be formatted before validation fails. The formatter never moves an `@let` out of its legal
prologue or changes list order.

Theme definitions use the section 26.4 schema order instead of requirement-domain sorting. Known
singleton fields and blocks move to their published positions. Palette leaves, Eza patterns and
categories, and every repeated application-map record remain in semantic source order. Unknown or
duplicate entries retain their relative source position so formatting remains total; validation
still rejects them.

A block is inlined only when it has no nested block, has at most three entries, has no comments,
and fits 100 columns. Otherwise it has one entry per line.

Comment attachment:

- a standalone comment attaches to the next entry unless separated from it by a blank line;
- a blank-line-separated comment is a section comment and remains before the following region;
- a comment after the last entry stays at block end;
- a trailing comment has two spaces before `#`;
- comments move with their attached entry during sorting.

The generated lock has the separate one-record-per-line canon in section 22. `dotfile fmt --check`
accepts it only to verify canonical bytes; a non-check `dotfile fmt package.lock.dotfile` rejects
the generated file and directs the user to `dotfile lock`. `dotfile format` formats `.conf` files;
it does not format `.dotfile` sources.

## 24. Diagnostics and validation order

Diagnostics are part of the conformance surface. Each diagnostic contains:

- stable machine-readable code;
- stage and severity;
- short summary and concrete remedy;
- tight primary span and related spans;
- qualified semantic path/identity;
- merge, occurrence, mapping, or import-free discovery provenance as applicable;
- structured expected/actual data;
- secret-redaction marker;
- optional structured fix-it.

Stage order is exactly `lex`, `parse`, `schema`, `theme`, `resolve`, `graph`, `discovery`, `deploy`,
`lock`, `bind`, `observe`, `apply`. The complete stable code registry is version-owned by stage:
the `.dotfile` version owns `lex` through `deploy`, the generated-lock version owns `lock`, and the
built-ins version owns `bind`, `observe`, and `apply`:

| Stage | Codes |
|---|---|
| `lex` | `lex/encoding`, `lex/token` |
| `parse` | `parse/syntax` |
| `schema` | `schema/context`, `schema/duplicate`, `schema/binding` |
| `theme` | `theme/discovery`, `theme/reference`, `theme/merge`, `theme/map`, `theme/output` |
| `resolve` | `resolve/reference`, `resolve/identity`, `resolve/resource-key`, `resolve/fact-conflict`, `resolve/adapter`, `resolve/theme` |
| `graph` | `graph/cycle` |
| `discovery` | `discovery/group`, `discovery/inventory`, `discovery/source` |
| `deploy` | `deploy/mapping`, `deploy/permission`, `deploy/collision`, `deploy/variant` |
| `lock` | `lock/stale`, `lock/noncanonical`, `lock/tampered` |
| `bind` | `bind/profile`, `bind/host`, `bind/variant`, `bind/destination` |
| `observe` | `observe/absent`, `observe/adapter`, `observe/vault`, `observe/destination` |
| `apply` | `apply/approval`, `apply/race`, `apply/rollback` |

The first applicable code in the corresponding stage is used; diagnostics may carry a structured
`detail` discriminator but MUST NOT invent another string code without revising the version that
owns that stage. A change spanning stages revises every affected owner. Independent errors are
collected and sorted by this stage order, path bytes, start byte, then code.
Conflict diagnostics show every origin, not merely the final one.

Theme-stage errors block `dotfile theme apply` and `dotfile theme check`. Syntax and generic-schema
errors in any theme source also block `dotfile fmt --check` for that source. Theme contents are not
generated-lock inputs, so a theme value error does not masquerade as a graph or deployment error.

### 24.1 Source compilation errors

These block `dotfile lock`:

- invalid UTF-8/BOM/control, malformed token, escape, interpolation, or path;
- unmatched delimiter, missing value, missing comma, or illegal separator;
- reserved word/attribute/block, wrong node context, or wrong value/reference type;
- `@let` outside a prologue, redeclaration, or use before declaration;
- unresolved group/profile/theme/extension reference;
- duplicate group/profile/host identity or hostname alias;
- resource without exactly one valid key;
- same-scope or co-active scoped-fact conflict;
- invalid adapter shape or unverifiable version;
- demand cycle active in a declared profile;
- group-directory overlap/symlink/absence;
- missing deployable tracked source, untracked ignore file, ignored source opted into deployment,
  unsupported source type, or unsafe source symlink;
- uncovered copy/system leaf, malformed symbolic destination, invalid mode/ownership, or
  deployment collision;
- plaintext leaf under private-only deployment;
- noncanonical or inconsistent variant declaration.

### 24.2 Warnings and informational output

These do not block compilation unless promoted by policy:

- package directory without `package.dotfile`;
- near-name possible entity typo;
- entity facts that are unused by every declared profile;
- empty deployable facet after exclusions;
- unused binding;
- departed node/facet compared with the previous lock.

### 24.3 Consumer and machine preconditions

These are not source errors:

- stale or tampered lock;
- absent installed required/optional entity;
- missing check-only repository path;
- incompatible selected profile/host;
- saved profile drift;
- missing/unknown group variant selection;
- foreign, modified, broken, or unsafe destination;
- destination that resolves to the physical repository or one of its descendants during binding;
- sealed/unavailable vault identity or unresolved template variable;
- privileged approval refusal;
- check adapter timeout/failure.

Required absence fails `check`; optional absence is reported but does not fail. A check MAY promote
an optional missing child to a warning when its parent reason is observed as present; that is a UI
heuristic, not a change to demand mode.

## 25. CLI surface

| Command | Normative role |
|---|---|
| `dotfile lock [--check]` | compile/validate sources and write or verify `package.lock.dotfile` |
| `dotfile fmt [--check] [paths...]` | canonicalize `.dotfile` sources; generated lock only in check mode |
| `dotfile link [profile] [--override group=name] [--dry-run]` | bind/apply user links and user materialized files; never system copies |
| `dotfile system status|diff|install` | inspect or separately approve/apply privileged copy rows |
| `dotfile status [profile]` | compare bound user/system plan with destinations and state ledger |
| `dotfile check [profile]` | lock freshness plus adapter, path, profile, variant, and deployment checks |
| `dotfile why <qualified-or-unique-name>` | occurrences, reasons, facts, coordinates, profiles, and deployment provenance |
| `dotfile graph [profile]` | resolved acyclic occurrence graph with required/optional modes |
| `dotfile packages <profile> --emit <installer> [--optional]` | emit that profile's deduplicated coordinates for one installer |
| `dotfile add / remove` | scaffold/retire a package facet and update source through the formatter |
| `dotfile theme apply|check|status|show|switch|outputs` | consume the typed theme domains from section 26.4 |

Command meanings are distinct:

- `dotfile sync` performs repository orchestration (pull/build/lock/link/restart policy) and may
  call `dotfile lock`; it is not the compiler itself.
- `dotfile format` formats `.conf` files; `.dotfile` sources use `dotfile fmt`.
- vault, scan, secret-edit, theme, and benchmark commands use their separate typed domains.

`why`, `graph`, and package emission use lock semantics only. Package emission requires the
explicit profile argument and never infers it from host or saved machine state. `link`, `status`,
`check`, and system commands additionally observe current machine state. `lock`, `fmt`, `add`, and
`remove` necessarily read source files.

## 26. Peripheral `.dotfile` domains

These domains use the lexer, generic grammar, string/reference rules, comments, and formatter, but
they do not contribute demand or fact graph IR. Theme profile identities additionally enter the
generated lock as described in sections 14 and 22; their definition trees remain theme-owned.

### 26.1 Recipient keys

`config/keys.dotfile` contains exactly one `recipients` block:

```dotfile
recipients {
    archpc = "age15wjewk6yjs5vsezah0sa9vz3gyl569eexwj74l8dvrc2vlsxuq3q7hq52d"
    recovery2 = "age1535gunzf00mxeww9v0ccerj5ygcthngm353hha8grwjqgh723qts99a3jm"
}
```

Labels match `[A-Za-z0-9][A-Za-z0-9._-]*`, are unique, and sort by bytes. Values are quoted strings
and MUST pass the registered age public-recipient syntax. Private identities are forbidden.

### 26.2 Secret-scan rules

`config/scan.dotfile` contains exactly one `allow` block with repeated rule blocks:

```dotfile
allow {
    rule { pattern = "scripts/python/tests/transcript/test_redact.py", inspect = "path" }
    rule { pattern = "shared/obsidian/plugins/**", inspect = "value" }
}
```

`pattern` is a quoted repository-relative glob. `inspect` is `"path"` or `"value"` and is required;
there is no silent extra-field discard. Rules preserve source order because order is diagnostic
presentation order, though matching results are set-like.

Glob matching is anchored to the complete NFC repository-relative path and is case-sensitive.
Patterns use `/` separators, have no empty, `.` or `..` component, and support only these
metacharacters: `*` matches zero or more non-`/` scalars, `?` matches exactly one non-`/` scalar,
and a component equal to `**` matches zero or more complete path components. Character classes,
brace expansion, backslash escapes, negation, and platform-native separators are invalid. A
literal `*` or `?` cannot be represented in a version 1 scan pattern.

### 26.3 Benchmark baselines

`benchmarks/baselines.dotfile` is generated host → run-ID data:

```dotfile
archie {
    10db7d1f = "2026-08-13T11-34-32Z-10db7d1f"
}
```

The outer name is a host `IDENT`. Each inner key is a benchmark epoch: exactly eight lowercase
hexadecimal digits derived by the benchmark producer from identity-bearing hardware fields. Its
quoted value is an immutable run ID matching
`[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}-[0-9]{2}-[0-9]{2}Z-[0-9a-f]{8}`; the final component MUST
equal the key. Derivation of the epoch is owned by the benchmark schema, not this language.
Duplicate hosts or epochs are errors instead of last-wins behavior. The benchmark store owns
canonical ordering by host bytes and then epoch bytes.

### 26.4 Theme definitions

All theme configuration interpreted by `dotfile theme` uses this closed `.dotfile` domain:

```text
theme/roles.dotfile
theme/fonts.dotfile
theme/profiles/<theme>.dotfile
theme/maps/catppuccin.dotfile
theme/maps/eza.dotfile
theme/maps/gtk.dotfile
theme/maps/kde.dotfile
theme/maps/obsidian.dotfile
```

The two fixed base files, all five fixed map files, and at least one profile are mandatory. They
MUST be tracked ordinary files, not symlinks. Profile files are immediate children of
`theme/profiles`; `<theme>` is an `IDENT` and is the theme's identity independently of its display
name. Files with an unregistered basename, nested profile directories, and theme-control files in
another format are errors. Native configuration generated for an application remains in that
application's format and is not a theme-definition source.

Theme files use only the existing string, reference, list, assignment, and block forms. They do
not add general numbers, Booleans, quoted keys, inline objects, or arbitrary tables to the
language. Decimal data is a schema-checked quoted string. Arbitrary external keys are represented
by repeated records with a quoted `key` field. Theme files forbid `@let`, interpolation, adjacent
string atoms, demands, paths, and deployment attributes. Every theme data string is one literal,
single-line `STRING` token in NFC.

#### 26.4.1 Shared roles and fonts

`theme/roles.dotfile` has these root blocks in this order when present:
`roles`, `terminal`, `eza`, `kde`, and `konsole`. `terminal` may contain `ansi` and `tabs`; `eza`
may contain `categories` and repeated `pattern` blocks. Every ordinary leaf is a bare palette
reference. An Eza pattern has exactly a quoted `key` and a bare `role`:

```dotfile
roles {
    section_system = blue
    section_hardware = peach
    sudo = red
}

terminal {
    foreground = text
    background = base

    ansi {
        black = surface1
        bright_black = surface2
    }
}

eza {
    fi = subtext
    di = blue

    pattern { key = "*.toml", role = orange }
    pattern { key = "*.json", role = yellow }

    categories {
        image = mauve
        archive = red
    }
}
```

`roles`, the direct scalar regions of `terminal` and `eza`, `terminal.ansi`, `terminal.tabs`,
`eza.categories`, `kde`, and `konsole` are open role maps: each accepts unique `IDENT` keys and bare
palette-reference values. The blocks themselves and the `pattern` record shape are closed. Thus a
new named role is preserved as typed data rather than silently ignored, while an unknown structural
block or record field remains an error.

`theme/fonts.dotfile` contains exactly `fonts`, `sizes`, and `applications`:

```dotfile
fonts {
    general = "Noto Sans"
    nerd = "Hack Nerd Font Mono"
}

sizes {
    terminal = "12"
    terminal_mac = "13"
    interface = "10"
}

applications {
    obsidian = "enabled"
}
```

Font families are non-empty one-line strings and MUST NOT contain a comma. The three shown size
keys are required; their values are positive canonical decimals. A canonical decimal is ASCII,
has no sign or leading zero except the value `0`, has no exponent, and when fractional has neither
an empty fraction nor a trailing zero. Application keys are `IDENT`s and values are exactly
`"enabled"` or `"disabled"`; after base/profile merging, an absent application key means
`"disabled"`. `fonts` is an open unique-`IDENT` string map with `general` and `nerd` required;
`sizes` accepts exactly the three shown keys; `applications` is an open unique-`IDENT` enum map.

#### 26.4.2 Theme profiles

Each `theme/profiles/<theme>.dotfile` contains the required root fields `display-name`,
`appearance`, and `icons`, followed by required `nvim` and `palette` blocks. `appearance` is
`"dark"` or `"light"`; `display-name` and `icons` are non-empty one-line data strings; `nvim`
contains exactly the non-empty string `flavour`. Palette keys are `IDENT`s and values are lowercase
`#[0-9a-f]{6}` strings:

```dotfile
display-name = "Catppuccin Mocha"
appearance = "dark"
icons = "Breeze Chameleon Dark"

nvim { flavour = "mocha" }

palette {
    flamingo = "#f2cdcd"
    pink = "#f5c2e7"
    mauve = "#cba6f7"
    red = "#f38ba8"
    base = "#1e1e2e"
}

terminal {
    ansi {
        black = subtext
        bright_black = overlay2
    }
}
```

A profile may sparsely override any leaf shape accepted by `roles.dotfile` or `fonts.dotfile` by
using the same block and field spelling after its palette. Resolution starts with the shared
role/font trees and replaces matching leaves with profile leaves. Replacing a leaf preserves its
base position; a new allowed leaf appends after existing siblings in profile source order. Absence
means inherit, and deletion is not supported. Palettes never inherit: every profile declares its
complete palette. “Complete” means it defines every palette name referenced by its resolved role
tree and every registered map; additional unique palette names are allowed.

Every profile, including an unselected one, is resolved and validated. Palette values are unique
within a profile. Across profiles, one hexadecimal value may recur only under the same palette
name; this makes reverse color remapping unambiguous. Every effective palette reference MUST
resolve, KDE role names MUST NOT shadow palette names, the required `general`/`nerd` fonts and all
three size fields MUST exist, and profile-local application values retain the exact
enabled/disabled type. Source validation resolves only references authored in the typed theme
domain; open role maps do not acquire an implicit required-key vocabulary from an application
renderer. If a renderer contract requests an undeclared role, that contract invocation fails with
`theme/reference` rather than inventing a default or retroactively making the source schema vary by
renderer.

#### 26.4.3 Registered application maps

Each map filename selects one exact schema. Entries in all repeated-record containers are ordered
data: duplicate keys are errors, and the formatter never sorts them.

`catppuccin.dotfile` contains one `colors` block of repeated entries mapping a lowercase six-digit
hex value without `#` to a palette reference:

```dotfile
colors {
    entry { key = "1e1e2e", palette = base }
    entry { key = "cdd6f4", palette = text }
}
```

`eza.dotfile` contains one `categories` block. Each `name` declares a local Eza-category identity
matching `IDENT`. Its `extensions` value is a non-empty ordered list of unique strings matching
`[a-z0-9][a-z0-9_+-]*`; a value contains no leading dot, slash, or whitespace. One extension may
belong to only one category:

```dotfile
categories {
    category {
        name = image
        extensions = ["png", "jpg", "jpeg", "gif", "svg"]
    }
}
```

Every key in `roles.dotfile`'s `eza.categories` role map MUST name a category identity declared
here. Map-only categories are allowed and unused until a role is assigned. This is a keyed join,
not a reference to a global entity or resource namespace.

`gtk.dotfile` contains one `colors` block. Each entry maps an external string key to a bare palette
or KDE-role reference:

```dotfile
colors {
    entry { key = "theme_bg_color", role = window_bg }
    entry { key = "error_color", role = negative }
}
```

`kde.dotfile` contains `groups`, `foregrounds`, and `selection-foregrounds` in that order. A group
entry has an external key and exactly two ordered references to keys in the resolved `kde` role
map. The other containers map an external key to one resolved `kde` role:

```dotfile
groups {
    entry { key = "Colors:Window", roles = [window_bg, window_alt] }
    entry { key = "Colors:Header][Inactive", roles = [window_bg, window_alt] }
}

foregrounds {
    entry { key = "ForegroundActive", role = active }
}

selection-foregrounds {
    entry { key = "ForegroundNormal", role = selection_fg }
}
```

`obsidian.dotfile` contains `derived` followed by `variables`. `derived` has exactly
`source = <palette-reference>`. Each ordered `variable` has a unique quoted CSS key and exactly
one of these value shapes:

- `palette = <palette-reference>`;
- `rgb = <palette-reference>`;
- `color = <palette-reference>` together with required `alpha = "<decimal>"`;
- `derived = <derived-reference>`;
- `literal = "<one-line data>"`.

Alpha is a canonical decimal in the inclusive range zero through one. The allowed derived
references are `accent_h`, `accent_s`, `accent_l`, and `accent_hsl`.

```dotfile
derived { source = mauve }

variables {
    variable { key = "--color-base-00", palette = crust }
    variable { key = "--color-red-rgb", rgb = red }

    variable {
        key = "--background-modifier-cover"
        color = crust
        alpha = "0.72"
    }

    variable { key = "--accent-h", derived = accent_h }
    variable { key = "--scrollbar-bg", literal = "transparent" }
}
```

Unknown structural blocks, closed-record fields, value-shape combinations, map names, or
unresolved references are errors; there is no ignored extension mechanism. Open role/palette maps
are explicitly typed containers, not an unknown-field escape hatch.

#### 26.4.4 Canonical ordering and merge details

The canonical root order of `roles.dotfile` is `roles`, `terminal`, `eza`, `kde`, `konsole`.
Within `terminal`, direct role leaves retain source order and precede `ansi`, then `tabs`. Within
`eza`, direct role leaves retain source order, followed by `categories`, then repeated `pattern`
records. The root order of `fonts.dotfile` is `fonts`, `sizes`, `applications`; open-map entries
retain source order, while size fields order as `terminal`, `terminal_mac`, `interface`.

The canonical profile root order is `display-name`, `appearance`, `icons`, `nvim`, `palette`, then
optional override blocks in this order: `roles`, `terminal`, `eza`, `kde`, `konsole`, `fonts`,
`sizes`, `applications`. Every open role, font, application, and palette map retains source order.
The five application-map files use the block order published in section 26.4.3, and every repeated
record retains source order. Comments remain attached CST trivia.

Profile leaf replacement preserves the base leaf's position, while a newly introduced allowed
leaf appends in profile order. Eza patterns merge by their decoded `key`: a matching profile key
replaces the role at its base position, and a new key appends in profile pattern order. A theme
file MUST NOT declare the same decoded pattern key twice in its own scope.

The resolver derives an ordered Eza rule sequence. For each assigned category in
`eza.dotfile` category order, it emits one semantic rule per extension in list order, using that
category's resolved role; it then appends explicit patterns in merged order. Explicit patterns
therefore have later-match precedence. This ordered semantic sequence, not application-specific
rendered bytes, is part of the resolver result.

The theme resolver parses all profiles, not only the selected theme, so the cross-profile palette
identity rules are checked globally. Its normative result is the ordered, fully merged typed theme
tree, the five ordered typed map trees, and the derived Eza rule sequence. The `rgb`, `derived`,
and other Obsidian tags are typed operation names; RGB-to-HSL calculation, application-file
rendering, emitter ordering, owned-region algorithms, output paths, and stageability belong to the
repository's renderer contract, not to `.dotfile` syntax or semantic resolution. This specification
therefore does not authorize a parser or resolver to rewrite native application files.

#### 26.4.5 Repository assignment and lock boundary

Repository-generated output must be assigned to a qualified facet by its renderer contract; group
or package identity is never inferred by splitting an output path. Given that facet, its
repository-output theme is selected without a machine profile:

1. use the facet's own `@theme` when present;
2. otherwise walk its declaration group, then declared parents from nearest to farthest, and use
   the first group-root `@theme` encountered;
3. for a non-`shared` declaration group, consider the `shared` group-root `@theme` next;
4. otherwise use the repository top-level `@theme`;
5. if none resolves, generation for that output is an error.

This source-only chain is deliberately distinct from the machine-bound precedence in section 14.
Sibling-group contributions, host themes, machine-profile themes, saved state, and one-invocation
CLI choices never change committed repository bytes.

`dotfile theme apply` validates the complete typed domain and delegates native-file rendering to
the separate renderer contract. `dotfile theme check` performs the same read-only source
resolution and renderer drift check.

Renderer freshness is a consumer precondition, not source-language validity and not a compilation
input. After pure compilation identifies its deployment candidates, the renderer contract supplies
the exact registered-artifact subset among those candidates. Before `dotfile lock` reads a member
of that subset as payload, and before `link` or `system install` applies a bound plan containing one,
the command MUST invoke the same read-only check scoped to exactly those artifacts. Drift in an
unrelated output or an inactive theme does not block the command. An unavailable contract, an
unregistered claimed generated artifact, or an indeterminate check fails with `theme/output`; the
command MUST NOT generate or modify an artifact as part of this precondition.

After `dotfile theme apply` changes a materialized deployment source, the generated lock MUST be
regenerated before the command reports a clean repository. Thus the final generated digest, never
stale pre-render bytes, enters the deployment candidate, and a later theme-source edit cannot make
an applying command accept that stale render merely because the old artifact still matches its
locked byte digest.

The generated lock stores theme identities and assignment provenance, not theme-definition trees.
Theme commands read this typed peripheral domain directly. Native renderer behavior, including its
deterministic output-to-facet registry and artifact-scoped freshness operation, is a fixed
repository integration outside language conformance and requires its own output-ownership and
golden tests. A parser/compiler claiming language conformance need not implement native rendering;
a destination command claiming repository integration MUST provide that fixed contract or refuse
plans containing registered theme-generated artifacts.

`dotfile theme switch <theme> <scope>` is a CST-aware source edit followed by generation. Repository
scope writes the top-level `@theme` in `config/profiles.dotfile`; group scope writes the group-root
`@theme`; facet scope writes that facet's `@theme`. An `everything` switch writes the repository
default and removes group/facet overrides only after explicit confirmation. It never removes host
or machine-profile declarations. `show`, `status`, and `outputs` are read-only views of the same
typed definitions, resolved assignments, and registered output inventory.

## 27. Complete worked source example

This example illustrates the complete source spellings and mappings.

### `shared/zsh/package.dotfile`

```dotfile
@description = "Shared Z shell configuration"

./.zshrc { @destination = "~/.zshrc" }

lazygit                              # gd alias
?ncdu { @description = "disk alias" }
starship
zsh                                  # explicit self-demand
zsh-autosuggestions
zsh-syntax-highlighting
```

Unmapped leaves still use the facet default under `~/.config/zsh`; `.zshrc` uses its longer exact
mapping.

### `shared/starship/package.dotfile`

```dotfile
@description = "Cross-shell prompt configuration"
@destination = "~/.config"

starship
```

`shared/starship/starship.toml` maps to `~/.config/starship.toml`, not a directory row.

### `shared/obsidian/package.dotfile`

```dotfile
@let vault = "~/Documents/main"
@description = "Obsidian theme generated from the active theme profile"
@destination = "${vault}/.obsidian"

./hotkeys.json { @destination = "${vault}/.obsidian/hotkeys.json" }

obsidian
```

The child mapping is semantically redundant here but legal; identical expansion deduplicates while
retaining both mapping spans. Directory-unfolding child entries are unnecessary because version 1
always emits leaves.

### `shared/wezterm/package.dotfile`

```dotfile
@description = "Terminal emulator with a modular Lua configuration"

./types {
    @deploy = "none"
    @expect = "directory"
}                                    # generated editor stubs: check, never deploy

@font {
    @key = hack_nerd_font
    @description = "Terminal font stack"
    @family = ["Hack Nerd Font Mono", "JetBrainsMono Nerd Font"]
}

tmux
wezterm { @version = "20260813-114614-18a44cb7" }
```

### `macos/package.dotfile`

```dotfile
brew = "homebrew"

@extend font/hack_nerd_font {
    @pkg = "font-hack-nerd-font"
    @installer = "brew-cask"
}

wezterm {
    hammerspoon
}
```

The extension contributes macOS coordinates without adding a second font demand.

### `linux/common/package.dotfile`

```dotfile
@extend font/hack_nerd_font {
    @pkg = "ttf-hack-nerd"
}

flameshot { @description = "Screenshot tool" }
?kitty {
    xremap
}
sysinfo {
    fc-cache = "fontconfig"
    ?glmark2
    ?sensors = "lm_sensors"
    wl-copy = "wl-clipboard"
    wl-paste = "wl-clipboard"
}
```

Both `kitty` and its lexical child are optional. The package emitter collapses the two
`wl-clipboard` coordinates but retains both reasons.

### `linux/arch/macie-usb/package.dotfile`

```dotfile
@description = "USB-C direct link to the Mac: interface naming, DHCP, NM opt-out"
@deploy = "copy"
@privilege = "system"
@owner = "root"
@group = "root"

./etc {
    @destination = "/etc"
    @mode = "0644"
}

dnsmasq { @description = "DHCP for the macie ↔ archie cable link" }
```

A source such as `etc/systemd/network/10-macie.link.tmpl` renders privately to
`/etc/systemd/network/10-macie.link`. It remains a system copy, but the transform locks that row to
`0600` rather than inheriting the plain-file `0644` default. This combines template rendering,
system-copy semantics, and a locked private mode.

### Override variant

```text
linux/hyprland/overrides/laptop/hypr-local/package.dotfile
linux/hyprland/overrides/laptop/hypr-local/local.conf
```

With group selection `hyprland=laptop`, the physical source above has logical facet
`facet:hyprland/hypr-local@laptop` and maps by default to
`~/.config/hypr-local/local.conf`. The lock stores both paths and the `laptop` tag.

### Representative lock rows

```dotfile
@nodes {
    node { id = "entity:wezterm", node_kind = "entity" }
    node { id = "resource:font/hack_nerd_font", node_kind = "resource", resource_kind = "font", resource_key = "hack_nerd_font" }
}

@occurrences {
    occurrence { id = "occ:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", target = "resource:font/hack_nerd_font", root = "facet:shared/wezterm", group = "group:shared", local_mode = "required", effective_mode = "required", source_span = "30:shared/wezterm/package.dotfile:100:105:5:1:5:6", reasons = [] }
}

@deployments {
    candidate { facet = "facet:arch/macie-usb", declaration_group = "group:arch", variant = "base", physical_source = "linux/arch/macie-usb/etc/systemd/network/10-macie-usb.link.tmpl", logical_source = "linux/arch/macie-usb/etc/systemd/network/10-macie-usb.link.tmpl", output_source = "linux/arch/macie-usb/etc/systemd/network/10-macie-usb.link", destination = "/etc/systemd/network/10-macie-usb.link", action = "copy", render = "template", privilege = "system", sensitivity = "private", mode = "0600", owner = "root", owner_group = "root", source_type = "regular", source_digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111", vault_source = "vars.enc.yaml", vault_digest = "sha256:2222222222222222222222222222222222222222222222222222222222222222", provenance = ["s:11:destination:36:linux/arch/macie-usb/package.dotfile:100:105:7:1:7:6"] }
    candidate { facet = "facet:shared/starship", declaration_group = "group:shared", variant = "base", physical_source = "shared/starship/starship.toml", logical_source = "shared/starship/starship.toml", output_source = "shared/starship/starship.toml", destination = "~/.config/starship.toml", action = "link", render = "plain", privilege = "user", sensitivity = "public", source_type = "regular", provenance = ["s:11:destination:31:shared/starship/package.dotfile:50:75:2:1:2:26"] }
}
```

The actual formatter keeps each generated record on one line. Digest/offset literals and the
single-item provenance lists above are abbreviated illustrations rather than conformance vectors;
a real row carries the complete field-origin union. No resource is flattened to a bare entity
block, and no package-directory deployment row is legal.

## 28. Conformance requirements

A conforming implementation MUST pass these fixture suites:

1. lexer fixtures for UTF-8, BOM, CRLF, comments, every escape, interpolation, sigil adjacency, and
   quoted paths;
2. parser fixtures for every grammar production, multiline/trailing separators, and the exact
   neighboring negative cases;
3. one schema fixture per allowed/forbidden entry and attribute context;
4. demand-occurrence fixtures for nested optionality, required-wins, provenance, and cycles;
5. fact-merge fixtures for shared baseline, descendant replacement, equal siblings, conflicting
   siblings, and list replacement;
6. per-profile manager/installer/check-resolution fixtures, including Arch and Ubuntu separation;
7. payload fixtures for Git selection, ignored/generated paths, symlinks, special files, transforms,
   exact mappings, and metadata exclusion;
8. variant fixtures for prefix stripping, base fallback, addition, switching, `none`, and invalid
   selection;
9. collision fixtures for identical, same-group, cross-group, cross-action, source-mapping overlap,
   destination-prefix, case, and Unicode cases;
10. canonical lock/JSON golden files and cross-implementation digest vectors;
11. formatter idempotence and comment-attachment fixtures;
12. safe-application tests for foreign paths, ledger pruning, stale/tampered locks, no-follow parent
    races, source-descriptor swaps, no-replace creation, guarded replacement refusal, private HMAC
    verification, rollback journals, permissions, redaction, and privilege-helper validation;
13. theme-domain fixtures for discovery, base/profile merging, every registered map shape,
    semantic order, Eza joins, application defaults, cross-profile palette identity, and a stubbed
    renderer-contract check proving that lock generation and destination application refuse
    exact-plan output drift without being blocked by unrelated output drift;
14. fresh-reader tests that correctly answer graph, manager, mapping, variant, lock, and mutation
    questions using this document alone.

Destination-mutating commands MUST satisfy parsing, compilation, lock freshness, binding, and the
section 21 safety requirements before making any filesystem change.

## 29. Research basis

The format borrows specific properties rather than cloning one general configuration language:

| Precedent | Adopted lesson | Explicitly not adopted |
|---|---|---|
| [CUE specification](https://cuelang.org/docs/reference/spec/) | order-independent conflict detection and multi-origin facts | full constraint lattice and implicit complex unification |
| [Dhall safety guarantees](https://docs.dhall-lang.org/discussions/Safety-guarantees.html) | pure/total configuration boundary and integrity-minded dependency design | higher-order typed calculus and remote imports in v1 |
| [Starlark specification](https://starlark-lang.org/spec.html) | deterministic, hermetic, finite configuration evaluation | statements, loops, host-injected effects, and general scripting |
| [HCL/Terraform syntax](https://developer.hashicorp.com/terraform/language/syntax/configuration) | parser/schema separation, arguments plus nested blocks, structured diagnostics | broad expression/template language and host-dependent semantics |
| [TOML 1.0](https://toml.io/en/v1.0.0) | obvious quoting, UTF-8/line rules, comma-delimited lists, duplicate rejection | tables as the semantic model and last/implicit merging |
| [Jsonnet language reference](https://jsonnet.org/ref/language.html) | immutable data and formal desugaring | lazy unchecked fields, `self`/`super`, inheritance layers, comprehensions |
| [Nickel merging](https://nickel-lang.org/user-manual/merging/) | provenance-rich conflicts and explicit metadata/default concepts | arbitrary merge priorities and recursive late-bound records |
| [Nix language documentation](https://nix.dev/manual/nix/2.30/language/index.html) | explicit functional separation between evaluation and realization | ambient inputs, computed imports, path interpolation effects, lazy validation |
| [GNU Stow](https://www.gnu.org/software/stow/manual/stow.html) and [Home Manager files](https://nix-community.github.io/home-manager/options/home-manager/home.html) | preflight conflicts and recursive leaf linking | tree folding that would expose co-located metadata |
| [Git ignore rules](https://git-scm.com/docs/gitignore) | repository-relative ignore precedence and pattern behavior | user-global and repository-private exclude sources as compiler inputs |
| [RFC 8785 JCS](https://www.rfc-editor.org/rfc/rfc8785.html) | byte-stable canonical JSON for hashing | ad-hoc delimiter hashing |
| [Unicode normalization](https://www.unicode.org/reports/tr15/) | explicit NFC/equivalence handling | silent compatibility normalization of arbitrary paths |

The central synthesis is intentionally conservative: CUE-like conflict behavior, Dhall/Starlark-like
purity, HCL-like schema-directed blocks, TOML-like obvious data, a first-class occurrence/provenance
IR, and a Stow/Home-Manager-informed leaf deployment plan with a stricter ownership boundary.

## 30. Reserved future features

The following spellings are reserved and invalid in `.dotfile` version 1:

- `@import` and all remote/local import syntax;
- `if`, `then`, `else`, `for`, `in`, and comprehension forms;
- `null`, Boolean/numeric literals, objects as values, and general operators;
- `@unset`, deployment whiteouts, and arbitrary merge priorities;
- arbitrary resource kinds, user-defined check adapters, or shell checks;
- remote package coordinates that cause the compiler to fetch content.

A future proposal must define its purity inputs, termination, type behavior, source maps,
canonicalization, lock representation, cycle behavior, security boundary, version transition, and
fixtures before changing the `.dotfile` version. Unknown reserved forms are errors, never ignored
extensions.
