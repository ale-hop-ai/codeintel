---
id: extraction
status: proposed
binding: no
---
# 02 — Extraction

Two tiers produce the facts in [01-facts.md](01-facts.md). This spec defines
what each emits, how they are joined, and what each is allowed to guess (almost
nothing).

```
                tier A — tree-sitter                 tier B — SCIP
                ────────────────────                 ─────────────
  runs          always, no build required            when index.scip exists
  cost          ~30s / 100k LOC                      whatever the indexer costs
  gives         defs, spans, signatures, docs,       global symbol identity,
                containment, imports                 references, implements, types
  precision     identifier text                      compiler front-end
  Prov          "name"                               "exact"
```

Neither tier is optional-in-principle: tier A is the skeleton (where things are),
tier B is the nervous system (what refers to what). Tier A alone gives a usable
map. Tier B alone gives references with no spans, no signatures, and no docs.

---

## Tier A — tree-sitter

### Adding a language is data — but the data is ours to write

**Superseded 2026-09-12.** The original claim was that upstream grammars ship
`queries/tags.scm` with a stable capture convention, so vendoring it plus a
small `imports.scm` was the entire per-language cost. All nine upstream files
were fetched and checked. The convention is not stable and vendoring does not
work:

| grammar | `@reference.call` | what breaks |
|---|---:|---|
| rust | 3 | `struct_item`, `enum_item`, `union_item` **and** `type_item` all capture as `@definition.class`, so every enum and type alias is emitted `Kind = "struct"` — an extractor guessing **wrong**, which invariant 1 forbids. `impl_item` is `@reference.implementation`, not a definition, so tier-A `parent` for every method is the file. Methods in a `declaration_list` match both `@definition.method` and `@definition.function` and emit two `def` rows. No `@definition.constant`. |
| go | 1 | emits `@definition.type`, which is absent from the kind table below; five patterns carry a bare `@name` with no tag at all, and emission is undefined for them |
| python | 1 | **no `@definition.method`** — every method is `Kind = "function"`, so `?- def(M,_,"method",N).` returns zero rows with `status: ok` |
| javascript | 2 | uses `#strip!`, `#select-adjacent!` — `tree-sitter-tags` directives, not core `Query` predicates; silently no-ops |
| **typescript** | **0** | captures only ambient `.d.ts` forms. `class X {}`, `function f() {}`, `const f = () => {}` produce **nothing** |
| **c / c++** | **0** | import-complete, call-empty |
| ruby | 2 | **no import node at all** — `require` is a method call, and Rails autoloads with none |
| java | 1 | `@reference.call` attaches to the *arguments* node, so recorded positions point at `(args)` |

So: **we author `tags.scm` and `imports.scm` per language.** Upstream is a
reference to start from, not a dependency to vendor. This budgeted ~1,000 LOC
of query and mapping per language; measured 2026-10-26 it is 97–207 lines of
`.scm` per language (Go 123, Python 97, Rust 127, TypeScript 207, `jsx.scm`
included) and no per-language Rust — the same order of *work* as
`~/projects/dex/internal/graph/sitter_rust_tags.go` and its siblings, which
spend ~6,177 LOC across five languages doing exactly this. That reprices M5 and
is why the language set is four, not nine.

`imports.scm` was always ours, which is why none of the above touches
`import/3` — and why architecture conformance is the capability that works on a
first run with no SCIP index.

The per-language cost is therefore:

```
queries/
  rust/tags.scm        ours
  rust/imports.scm     ours, ~10 lines
  go/tags.scm
  go/imports.scm
  ...
```

`tags.scm` capture convention. Upstream honours the first three; the rest are
ours, because the questions they answer are ones upstream does not ask.

| Capture | Meaning |
|---|---|
| `@definition.<kind>` | the node is a definition; the capture suffix is the `Kind`, after the language's `kind_remap` |
| `@name` | the identifier token within that definition |
| `@reference.call` | a call site |
| `@scope.<kind>` | the node **owns** definitions but is not one. A Rust `impl Store` declares nothing that `struct Store` did not; it contributes the `Store#` descriptor and no `def` row |
| `@owner` | an identifier **naming** this definition's owner, for a language where the owner does not enclose it |
| `@doc` | the node whose text **is** this definition's documentation, for a language that keeps it inside the definition rather than in comments before it |
| `@value` | the node a binding is initialized with. A definition whose `@value` has one of the language's `function_values` node kinds is a `function` |
| `@trait` | the implemented interface, on a `@scope.impl` node. The grammar's final identifier, as written |
| `@target` | the implementing type, on a `@scope.impl` node, when it says more than `@name` does |

`impl` is a **reserved `@scope` suffix** and the only capture suffix the
extractor reads by name (§ `@scope.impl`); every other suffix is a `Kind`.

Every node carries at most one `@definition.*` or `@scope.*` capture, asserted
by a test over every registered language.

#### `@owner`, and why span nesting is not enough

Span nesting answers "who contains this" and for Rust that is also "who owns
this", once `impl` blocks are scopes. Go breaks the identification: a method is
declared at file scope and its owner is named in its own receiver.

```go
func (s *Store) Get(key string) (Entry, bool)
```

Nothing encloses `Get`. Span nesting parents it to the file, which is not
merely imprecise — it is **wrong at the symbol level**, because the synthesized
symbol becomes `local store.go Get().`, so a second type with a `Get` method in
the same file collides onto the same symbol and emits two `def` rows for one
identity.

So a language may capture `@owner` on an identifier inside the definition.
Resolution is deliberately narrow, and it is still extraction, not inference —
the owner's name was read out of the source, not guessed:

1. The candidates are the definitions **in this same file** whose `Kind` is
   type-like (`struct`, `enum`, `trait`, `class`, `interface`, `type`) and
   which no other definition encloses.
2. Exactly one candidate matching `@owner`'s text wins and becomes the parent,
   for descriptor synthesis and for the `parent` row alike.
3. **Zero or more than one, and there is no override** — span nesting stands
   and the answer degrades to the file. An extractor that guessed between two
   candidates would violate invariant 1.

Restricting candidates to unenclosed definitions is what keeps the parent graph
a forest: the only edge `@owner` adds runs from a nested definition to a
top-level one, so `type Store struct{}` declared *inside* a method body can
never become that method's own owner.

Tier B overrides all of this where it has an answer (§ Parent precedence): the
compiler knows the receiver's package-qualified type and we do not.

#### `@doc`, and why the preamble walk is not enough

`def_doc` is read from the comment run immediately before the definition
(§ Emission), which is where Rust and Go keep it. Python does not: a docstring
is the first statement **of the body**, inside the node the definition captures,
and a `#` comment above a `def` is not documentation to any Python tool.

So a language may capture `@doc` on the node whose text is the documentation.
Python's `tags.scm` captures the `string_content` of a string that is the first
statement of the body, so the delimiters are never part of the text:

```scheme
(function_definition
  name: (identifier) @name
  body: (block . (expression_statement (string (string_content) @doc))?))
  @definition.function
```

The capture is optional inside the one definition pattern rather than a second
pattern, because a second pattern would match a documented function twice and
emit two `def` rows for it. The text gets the same per-line strip a comment
gets, with no marker — a docstring's lines are indented to the body. A language
states one mechanism or the other, in its query and in its `doc_markers`; the
two never compete for one definition.

#### `@value`, and why the keyword is not enough

`const make = (n) => n` says `const`, and what it binds is a function. The
grammar states that in the value node's kind, and `calls` counts references to
callable kinds only, so a `constant` there would hide every arrow helper and
component from the call graph.

A query cannot choose a capture suffix by a child's kind without enumerating
every other kind that child could be. TypeScript's value is any of 37
expression kinds, and a binding sits in one of six statement shapes, so doing
it that way would put hundreds of lines of kind lists in `tags.scm` with no
fact behind them. Go's fall-through type enumerates, and there the list is
eleven kinds, once. So a binding pattern captures its value node as `@value`,
and the language's `function_values` row in `lang.rs` names the node kinds that
make the definition a `function`. The rule is in the extractor once, and the
kinds are data. A class field with an arrow value stays a `field`: `@value` is
captured only on bindings.

#### `Export`, and why a query predicate is not enough

A tree-sitter predicate *filters a match*; it cannot *label* one. There is no
"capture this if the text matches" operator, so expressing visibility in
`tags.scm` means writing each definition pattern twice — once with the
public-making child and a `#eq?`, once without — and two patterns matching one
node emit two `def` rows for it. That is the upstream defect this file already
names twice (§ `@doc`, and `promote` in `tier_a.rs`), and the reason `@name` is
read by the extractor rather than asserted by the query.

So the predicate lives in the extractor once, as a closed vocabulary of
*mechanisms*, and which mechanism a language uses is data in its `lang.rs` row
([`Export`](../crates/extract/src/lang.rs)). It is a vocabulary in the sense
`Kind` and `Role` are: new languages pick from it, they do not extend it by
default.

| Variant | What it reads | Languages |
|---|---|---|
| `ChildWord { kind, words }` | a direct child of the definition node whose **text** is one of `words` | Rust (`visibility_modifier` = `pub`); Java, C#, Kotlin, Swift would reuse it unchanged |
| `Capitalized` | the first character of `@name` | Go |
| `NotUnderscored` | `@name`'s leading `_`, dunders excepted | Python |
| `Statement { .. }` | the definition's own **enclosing** statement, through a bounded set of `through` kinds, plus a class member's modifier child | TypeScript, TSX |

The first three are predicates over text the query already captures, and a
language that needs one adds a row, not code. `Statement` is the one that could
not be a query at all: tree-sitter patterns match **downward**, and
`export const a = 1` puts the exported-ness two or three ancestors above the
declarator that carries `@name`. Writing it downward enumerates
wrapper × `through`-chain × declaration shape, which is why that section of
`queries/typescript/tags.scm` is already the longest in the tree.

**This is the exception to "adding a language is data".** A language whose
visibility is stated by a mechanism none of the four describe — a Ruby
`private` section, which is a statement that changes the *default* for
everything after it — needs a fifth variant, and that is a Rust change. The
bar: a new variant must be parameterized by data and usable by more than the
language that motivated it, and it is added **here** in the same commit. A
`match lang.name` is still forbidden, and none exists.

#### `@scope.impl`, `@trait` and `@target`

`impl` is a **reserved `@scope` suffix**, not a Rust keyword that leaked. A
`@scope.impl` node is a *qualified* scope: it owns definitions, it is not one,
and two of them over the same type must give their members different symbols.
Three things follow from the suffix alone, in language-neutral code:

1. `@trait` and `@target`, both read as the grammar's final identifier, become
   `name_impl(F, Type, Trait, Line)` — unresolved, both as written.
2. The qualifier is pushed into the members' descriptor chains and not into the
   scope's own, so `parent` still names the type
   ([01-facts.md](01-facts.md) § Symbol identity).
3. A member of a qualified scope gets `visibility` = `inherited`: the language
   gives it no slot to state one. This is why the suffix exists apart from
   `@scope.type` — an inherent `impl` carries the same `Kind` and its members
   *may* state a visibility.

Any language with trait-like implementation blocks uses the same three
captures. Only Rust's query uses them today.

The qualifier is **one `[...]` segment per captured name**, never one segment
holding a joined phrase ([01-facts.md](01-facts.md) § Symbol identity). It
shipped joined — `` [`Handler for &Key`] `` — and the ` for ` in it was Rust's
surface syntax inside a `SymId` (#86). The segments come from the captures, so
a language whose implementation blocks read any other way needs no separator of
its own.

#### TypeScript and TSX: two grammars, one query

`tree-sitter-typescript` ships two grammars, registered as two languages:
`typescript` (`.ts`, `.mts`, `.cts`) and `typescriptreact` (`.tsx`), the SCIP
`Language` names ([01-facts.md](01-facts.md) § Atom vocabularies). JavaScript is
not registered. TSX's node set is TypeScript's plus JSX, and a JSX pattern does
not compile against the TypeScript grammar. So `queries/typescript/` is written
once, against TypeScript, and `queries/typescript/jsx.scm` holds only what JSX
adds. The `typescriptreact` row's tag query is the two concatenated. Nothing
is written twice.

A JSX element whose tag starts with an upper-case letter is a
`@reference.call`: a use of a component at that position, as a call is. A
lower-case tag is an intrinsic element and not a name in scope. That is the
rule the TypeScript compiler applies to JSX, read from the name exactly as Go's
export rule is. `<ui.Row />` is recorded by its final name, as `ui.row()` is.

Four things the TypeScript query decides, all from the node it matches:

- **An `export { x }` clause is a `name_export`, not a `visibility`.** The
  declaration of `x` is not under the `export_statement`, so `Export::Statement`
  correctly says `restricted`; the clause is a second statement in the file,
  captured in `imports.scm` as `@export` by its `export_specifier`'s `name`
  field (never `alias`), and for `export default f` and `export = f` by the
  bare identifier. `!source` keeps `export { x } from "m"` out: that one is
  an `@import`. The join to the declaration is `exported`'s third clause.

- **A binding whose value node is a function is a `function`.** In
  `const make = (n) => n`, the grammar states the value is an `arrow_function`,
  and `calls` counts references to callable kinds only, so recording it as a
  `constant` would hide every arrow helper and component from `calls`. The
  function kinds are `arrow_function`, `function_expression` and
  `generator_function`. Any other value is `constant` under `const` and
  `variable` under `let` or `var`. The query does not make this choice
  itself (§ `@value`).
- **A binding is a definition only at module or namespace scope.** A `const`
  inside a function body is a local, as a Python assignment inside a `def` is.
  The same goes for a method: `method_definition` is a definition inside a
  `class_body`, not inside an object literal.
- **`constructor` is the `constructor` kind**, by the name the grammar gives
  it, and `get x()` and `set x(v)` are methods named `x`
  ([01-facts.md](01-facts.md) § `def` for the two declarations one symbol then
  has).

Example, `tree-sitter-rust/queries/tags.scm`:
```scheme
(struct_item      name: (type_identifier) @name) @definition.class
(function_item    name: (identifier)      @name) @definition.function
(trait_item       name: (type_identifier) @name) @definition.interface
(mod_item         name: (identifier)      @name) @definition.module
(macro_definition name: (identifier)      @name) @definition.macro
```

Our `imports.scm` for the same grammar:
```scheme
(use_declaration argument: (_) @module) @import
(extern_crate_declaration name: (identifier) @module) @import
```

Kind mapping is a table, not a `match` arm per language:

| tags.scm capture | our `Kind` |
|---|---|
| `definition.function` | `function` |
| `definition.method` | `method` |
| `definition.class` | `class` (Rust/Go: `struct`, see § per-language overrides) |
| `definition.interface` | `interface` (Rust: `trait`) |
| `definition.module` | `module` |
| `definition.macro` | `macro` |
| `definition.constant` | `constant` |
| anything else | `unknown` |

Per-language overrides live in one `lang.rs` table: grammar, query paths, kind
remaps, export predicate, doc-comment prefix. ~20 lines per language.

### Emission

Per file, one parse, then:

1. Run `tags.scm`. Every `@definition.*` capture yields a `def` + `def_span`
   (the capture node's byte range) + `def_name` (the `@name` node's position).
2. **Containment by span nesting.** Sort definition spans by `(start_byte,
   -end_byte)`. Sweep with a stack: a definition's `parent` is the innermost
   enclosing definition, or the file path if none. This is the one non-trivial
   algorithm in tier A, it is ~30 lines, and **tier B reuses it verbatim** (§ Tier B).
   A definition carrying an `@owner` capture that resolves overrides its swept
   parent here, before any symbol is synthesized (§ `@owner`).
3. `def_sig` = the definition's header, whitespace-collapsed, capped at 512
   bytes. It is a display string, not a parse, but **where it ends is taken
   from the parse**: a node with a `body` field ends its header at the last
   named child in front of that body. `body` is tree-sitter's own field name
   in every grammar here, so this is not per-language Rust, and the
   punctuation that opens the body — Python's `:`, Rust's and Go's `{` — is an
   anonymous child between the two and drops out on its own.

   A node with **no** `body` field — an overload, an interface method, a type
   alias, a binding — states no such boundary, so the header is scanned
   instead: source from `start_byte` to the first body delimiter (`{`, `:` +
   newline, `=`, or end of line) at bracket depth zero, so `struct S<T = u32>`
   is not cut at its `=`. That depth is lexical, with one exception: a `<` or
   `>` with `-` or `=` immediately beside it is half of `<-`, `->`, `=>`, `<=`
   or `>=` and moves nothing. Without it Go's `func f(c <-chan int) error {`
   never returned to depth zero and the header ran to end of line, and
   TypeScript's `(cb: (x: number) => void, o: {a: 1})` reached depth zero at
   the `=>` and was cut mid-parameters (#43).

   The scan is the fallback and not the rule because three spellings defeat it
   and **no lexical rule can fix them** (#74): `>>` closes two generic
   arguments in Java and C++ (`docs/plan.md` defers both, it does not reject
   them), `<<` opens two qualified paths in Rust — `fn f(x: <<A as B>::C as
   D>::E)` compiles — and a bare `<` is a generic opener and a comparison in
   the same grammar. Choosing between them from characters is a guess, and
   invariant 1 forbids guessing. The grammar already knows, so the grammar is
   asked. Switching every body-bearing definition over moved no golden row in
   any fixture, which is the evidence that the two agree wherever the scan was
   right in the first place. The argument is correctness, not cost: the node
   lookup's cost against the scan's is unmeasured, and `docs/plan.md` M6 is
   where measuring happens.
4. `def_doc` = contiguous comment lines immediately preceding `def_span.start`,
   marker prefixes stripped. Emitted only if non-empty. A language whose
   documentation lives inside the definition captures it with `@doc` instead
   (§ `@doc`).
5. `visibility(S, Vis)` per the language's visibility predicate — one row for
   every definition, always. The extractor reports what the definition *states*
   (`public`, or `restricted` when it states a bounded visibility or states
   nothing where the grammar offers the slot), or `inherited` when the node kind
   has no slot at all. It does **not** walk ancestors to decide whether the
   symbol is reachable from outside: `exported` is a rule over `parent`
   (`01-facts.md` § Derived relations), and an extractor answering it here would
   be inferring, which is invariant 1. `@scope.*` items define nothing and get
   no row.
6. Run `imports.scm` → `import` rows. The module specifier is captured **as
   written**; no resolution. Where a language writes that specifier as a string
   literal, the row carries the literal's **content** — the span between the
   delimiter tokens the grammar already gives us, read from the tree and not
   matched out of the text ([01-facts.md](01-facts.md) § `import`). The same
   query's `@export` captures →
   `name_export(F, Name)` rows, one per local name an `export { }`,
   `export default` or `export =` clause names
   ([01-facts.md](01-facts.md) § `name_export`). The two live in one query
   because both are statements about what a file says at its edge, and an
   `export ... from` is already there as an `@import`.
7. `@reference.call` captures → `name_ref(Name, F, Line, Col, From)` rows, with
   `From` from the same span sweep. **Tier A does not resolve names to symbols.**
8. `@impl` captures → `name_impl(F, TypeName, TraitName, Line)` rows, both names
   as the grammar's final identifier. Same rule as 7: the names are recorded,
   not resolved. An inherent `impl` has no trait and emits nothing.

### Tier A does not resolve names

Every tier-A fact is a function of **its own file and nothing else**. A name
occurrence becomes `name_ref(Name, F, Line, Col, From)` — the text, its
position, and its enclosing definition. Which symbol that name denotes is
decided by rules in `stdlib.dl` ([01-facts.md](01-facts.md) § Derived
relations), evaluated over the whole repo at query time.

This is not a stylistic choice. A resolution decision depends on what exists in
*other* files, so freezing it into a per-file fact makes incremental indexing
unsound: adding a second `foo` in file B invalidates a fact in file A, and file
A will never be re-extracted because file A did not change. Per-file facts must
be per-file functions.

The secondary benefit is that the resolution policy is four readable rules a
user can tighten, loosen, or delete, rather than a buried Rust decision.

**We do not compete with compilers.** dex tried — per-language import tables, a
TypeScript constructor-dependency-injection special case — and still produced a
measurably skewed graph ([research.md](../docs/research.md) §1c). Tier A records
what the grammar saw. Tier B records what the compiler knows. The `Prov` column
on the derived `ref` says which one answered.

Explicitly *not* attempted anywhere in tier A: receiver/method dispatch
(`x.method()`), interface dispatch, generic instantiation, re-export chains,
dynamic dispatch, aliased imports crossing files. In Rust, Python, TypeScript,
and Java `x.method()` is the dominant call form, so **tier A alone yields a
sparse and unrepresentative call graph.** That is stated plainly here, in
`status` output, and in the README, because a sparse call graph reporting
`status: ok` is indistinguishable from code that has no callers.

### A file that does not parse is partial, and says so

tree-sitter is error-tolerant: a file with a syntax error still yields a tree,
and the extractor emits facts from it. What the grammar recovered is real
(invariant 1 holds) but **what it did not recover is not "absent from your
code"**. Measured (#131), one broken declaration and a valid one after it:

| language | error | valid declaration after it |
|---|---|---|
| Rust | four shapes (bad params, bad expr, unclosed struct, bad `if`) | kept, all four |
| Go | `func B( {`, unclosed `struct`, `var x =` | **lost**, three of four shapes |
| Python | `x = (1,`, `class K(:` + unclosed `[` | **lost**, two of three shapes |
| TypeScript/TSX | `function b( {`, unclosed class method, unclosed `{` | **lost**, three of three |

Whether the tail survives depends on the grammar and on where the error falls,
so no language can be declared safe and the extractor does not try to guess.
It reports the one thing the parser knows: the tree has an error.

- When `root.has_error()`, the file's `syntax_error` is the 1-based line of the
  first `ERROR` or `MISSING` node in document order. Otherwise it is absent.
  This is read off a node's byte range, so invariant 1 holds; no row is added,
  dropped or changed — facts are emitted exactly as before.
- It is a **per-file manifest field, not a relation**
  ([04-storage.md](04-storage.md) § Manifest). The relation budget is 16/16 and
  this needs no query over it; a `parse_error` relation would be the way to let
  a rule ask "is the file I am asking about whole" and is an owner decision
  (#131 option B).
- A file of the language that fails to parse at all (`Parser::parse` returns
  `None`) is still an extraction error, unchanged.

### Budget

Skip a file if: binary, > 2 MB, matched by `.gitignore`/`.ignore` (via the
`ignore` crate), or its grammar is unregistered. Skipped files produce **zero**
facts — not a `file` row — so `!file(F, _)` means "not indexed", which is
different from "indexed and empty".

**The ignore rules bound the whole index, not just tier A.** The walk is this
tool's only statement about which paths belong to the tree, so a SCIP document
naming a path the walk does not reach — ignored, hidden, or behind a symlink the
walk does not follow — is skipped exactly as an ignored source file is: no
`file` row, no tier-B-only `def`, no `scip_ref`. Enforcing the budget in tier A
alone re-admitted, through tier B, every generated artifact the indexer happened
to cover: on a `yarn` monorepo whose `.gitignore` holds `packages/*/build`,
`scip-typescript` covers the generated `build/**/*.d.ts` and each one came back
as a second `def` for an export already indexed from its source (#54). The
budget is per *path*, and the walk is what decides it. Each such document is
counted and named with the other skipped documents in the `index` summary
([05-surface.md](05-surface.md) § `index`); it is never silent.

**The walk runs on four threads, and that is why its outputs are ordered.**
Traversal and ignore-matching are the dominant cost of a warm call —
[plan.md](../docs/plan.md) § #28 measured the walk as the dominant phase of
`refresh` on a ~831k-LOC tree, against a per-file stale check of a few
milliseconds (that section withdraws its own magnitudes as not quotable, so none
is repeated here) — and they parallelize without touching what the walk decides. Each thread applies
the same per-entry policy and accumulates privately; the shares merge once per
thread, not once per file.

Determinism is preserved by construction, not by luck: `files` and the
unreadable list are sorted after the merge, and every other output is
order-independent by type — a set of paths and a map of counts per extension.
That is the same requirement [plan.md](../docs/plan.md) M2 already imposed, that
shuffling the file order changes nothing, so parallelism asks for no new
property. A test walks a wide tree twenty-one times and compares every field.

Four and not one per core: the walk is bound by directory reads rather than by
CPU, so beyond a handful of threads they contend and the time goes back up. Four
was the best of 2, 3, 4, 6 and 8 on all three pinned corpora.

---

## Tier B — SCIP

### Acquisition

By default `codeintel` reads `index.scip` if present, at `--scip <path>` or the
default `./index.scip`. It does not produce one.

**`codeintel index --run-indexers` is not built** ([docs/plan.md](../docs/plan.md)
M3, § Explicitly deferred). The command printed today is each registered
language's `indexer` field in `crates/extract/src/lang.rs`. The table and the
rules below are the design, kept for when it is built: detect the
languages present, look up the canonical indexer in a static table, check the
binary is on `PATH`, and shell out. The whole feature would be a table, a
`which`, and a subprocess.

| Language | Command | Needs |
|---|---|---|
| Rust | `rust-analyzer scip .` | a cargo workspace. Measured 2026-10-20 on fd and tokio: the role bitset is `Definition` or empty — every reference is `unknown`, so `Role` carries nothing — and no `Relationship` is written, so `scip_impl` is empty and `implements/3` is name-only |
| TypeScript/TSX | `scip-typescript index --infer-tsconfig` | `node_modules` installed, for anything beyond the project's own sources. `scip-typescript` 0.4.0 writes no `kind`, no `display_name`, no `Document.language`, an empty role bitset on every reference, and no position encoding while counting UTF-16 (§ Position normalization). It does write `is_implementation` for `implements`, so `implements/3` has TypeScript rows |
| Python | `scip-python index .` | an environment with deps. No `--project-name`: it defaults to empty, which upstream says supports repository-local navigation only — all tier B uses — and no name in the tree licenses a guess. The command is pasted verbatim, so it holds no `<placeholder>` (a redirect in `sh`); `--output` already defaults to `index.scip` |
| Go | `scip-go` | a buildable module. Measured on a 116-package module: `is_implementation` is written (476 relationships), so `scip_impl` has rows. Every reference carries `ReadAccess` and nothing else — the indexer hard-codes it (`internal/visitors/visitor_file.go`, `newReference`), so `read` means "is a reference" and `write`/`import`/`test` never occur. Installed from `github.com/scip-code/scip-go/cmd/scip-go` — the project moved out of the `sourcegraph` org, and the old path now fails `go install` with a module-path conflict rather than a 404 |
| Java/Scala/Kotlin | `scip-java index` | a working build |
| C / C++ | `scip-clang --compdb-path compile_commands.json` | a compilation database |
| Ruby | `scip-ruby` | a Sorbet-typechecked project |

Rules that keep this from becoming a build system:

- **Never implicit.** Without the flag, nothing is executed. An indexer can run
  arbitrary build code; that needs an explicit opt-in every time.
- **No environment management.** We do not install indexers, create virtualenvs,
  run `npm install`, or repair builds. A missing binary or a failing indexer is
  reported with its stderr and the exact command, and indexing continues with
  tier A only. It is never fatal.
- **The table is data.** Adding a language adds a row, not a code path.
- **Timeout, default 600 s**, after which the child is killed and reported.

Without the flag, `index` and `status` still **print the exact command** for
each detected language. Not "no SCIP index found" — the literal, copy-pasteable
line. The gap between tier A and tier B is the difference between a symbol map
and a call graph, and a user must never have to go looking for how to close it.

`codeintel status` reports SCIP presence, the emitting tool from
`Metadata.tool_info`, file coverage (`documents` matched against indexed files),
and staleness (index mtime vs. newest source mtime).

Multiple `index.scip` files (polyglot repos, one per language) are supported:
`--scip a.scip --scip b.scip`. They are ingested independently; symbol strings
are unique across indexes by construction so no merge logic is needed. Within
one index they are not always unique across *documents* — rust-analyzer keys by
package, not cargo target, so `crate/` and `main().` are defined once per
binary, test and example — and § The anchor join says what happens then.

Three sets are **recomputed over the merge**, never summed from the inputs:
the collisions (a symbol two inputs each define once is a collision), the
symbols the join may adopt (§ Parent precedence), and `extern`. "Not defined
by any document" is a statement about the whole merge: a symbol `a.scip`
only references and `b.scip` defines is a definition, and reading it as
`extern` because `a.scip` alone could not see its definition made one symbol
both `def` and `extern` (#47).

#### Document paths, and `--scip-root`

`Document.relative_path` is specified as relative to the project root, and
every fact this tool emits is keyed by a repo-relative path. The two are the
same string only when the indexer was run from the root this index covers.

They routinely are not. In a monorepo whose Rust crate lives in `daemon/` with
no workspace manifest at the top, `rust-analyzer scip .` has to be run from
`daemon/`, and it writes `src/lib.rs` where the walk has `daemon/src/lib.rs`.
Every document then misses every indexed file, and the result is zero anchors,
zero `scip_ref`, zero `resolved` — a tier-A-only index wearing a SCIP input
(#60).

**`--scip-root <DIR>`** declares, repo-relative, the directory an input's
document paths are relative to: `daemon` + `src/lib.rs` = `daemon/src/lib.rs`.
`.` is the repository root itself, the right value for the one input in a
polyglot set that was run from there (#102).
The re-based path is what § Position normalization reads source from, what the
anchor join keys on, and what every emitted fact carries. `escapes_root` is
checked **after** re-basing, so a root that climbs out of the repository is
refused like any other unusable document rather than producing a path no reader
can open.

It is repeatable and positional against `--scip`: zero roots means every input
is relative to the repository root, one root applies to every input, and N
roots pair with N inputs in the order given. Any other count is an error, never
a silent reuse of the last one — a mis-paired root would re-base a whole index
to the wrong subtree and still report a plausible anchor rate.

The root is recorded per input in the manifest and a change to it re-ingests,
exactly as a changed input does. A re-base is a different reading of the same
bytes, and mtime and size cannot see it.

#### Detection suggests; it never re-bases

When no document matched an indexed file, ingest looks for a prefix `P` such
that **every** document path `D` has `P/D` indexed, and names it in the report.
It does not apply it. Two reasons, and the first is binding.

- **Invariant 1: facts are extracted, never inferred.** A re-based path is a
  SCIP field plus a declaration. With no declaration it is a SCIP field plus a
  guess that happened to typecheck against the tree we walked today. Add a
  vendored copy, delete a crate, and the same index re-bases somewhere else —
  and every fact would move with it, with nothing in `Prov` able to say that a
  path was chosen rather than read. `Prov` has two values and neither of them
  means "inferred location".

- **The evidence is weakest exactly where it is needed.** Uniqueness is checked
  against the current walk, not against the tree the indexer actually saw. A
  monorepo with `daemon/src/lib.rs` and `agent/src/lib.rs` and a one-document
  index has two valid prefixes and no way to choose between them; a monorepo
  with one crate has exactly one valid prefix and still no way to know it is the
  one the indexer meant. "Provably unambiguous" is only ever provable about the
  wrong tree.

**Per-document suffix matching is worse, and is refused outright.** Letting each
document find its own unique match makes the tool decide which file a document
means, one document at a time, with no uniform rule to state afterwards. That is
the guess invariant 1 forbids, written once per document instead of once.

What detection buys is one line of report. The user reads it, re-runs with
`--scip-root daemon`, and the re-base is then a fact they declared. That is the
whole fix in practice, and it costs the facts nothing.

### Indexer evidence

`codeintel` prints a command for four programs it does not own. A printed
command reads as verified whether or not anyone ran it, and three bugs came
from that: a flag nobody had run (#93), a chain that cannot be written without
each indexer's real flag spelling (#102), and an indexer that exits 0 over an
empty index (#109). The audit is #122.

The honest place for the difference is **this table, not the hint.** "Never run
by us" is a fact about the project's knowledge, not about the repository: it
traces to no byte range and no SCIP field (invariant 1), it is constant per
language, and it changes only when someone runs an indexer. Carried in output it
would ride every `no-scip` and `scip-stale` answer, cannot sit inside the
runnable chain (`scip-python index . (unverified)` is a syntax error in `sh`,
and `# unverified` comments out the `&&` tail), and gives the reader — who
cannot verify either — a caveat with no action. What *did* run is reported where
it is knowable: `index` prints `scip: <path> (<tool> <version>) — N documents`.

Two classes and no gradient: **executed** — run, with tool version and date —
and **unverified**, which is everything else, a README included.
`crates/codeintel/tests/indexer_evidence.rs` fails when a `Lang.indexer` string
has no row, when a row names a command nothing emits, or when `executed` has no
version and date. Changing a command is changing its row, in the same commit.

| Command | Class | Evidence |
|---|---|---|
| `rust-analyzer scip .` | executed | 1.98.1, 2026-10-21, from the repository root of a copy of `tests/fixtures/rust`: exit 0, writes `./index.scip`. Also fd and tokio, 2026-10-20 (§ Acquisition) |
| `scip-go` | executed | 0.2.7, 2026-10-21, **only with `go.mod` in the working directory**: exit 0, `./index.scip`. With `go.mod` one level down it exits 0 too, warns `No go.mod file found`, writes a 172-byte index, and `codeintel index` ingests `0 documents` (#109), so the chain's `&&` does not stop it |
| `scip-python index .` | unverified | not installed on this host. `scip-python` 0.6.6 produced the committed fixture, on another worktree, with a command line nobody recorded (`Metadata.tool_info` has no arguments); its symbols name package `fixture 0.1.0` from no manifest in the fixture, so it was probably given flags this command does not carry |
| `scip-typescript index --infer-tsconfig` | unverified | not installed on this host. `scip-typescript` 0.4.0 produced the committed fixture with an unrecorded command line, and the fixture has its own `tsconfig.json`, so `--infer-tsconfig` was probably not exercised |

`codeintel index`, `--rebuild` and `--scip-root` in the same hints are this
binary's own flags and are not in the table. The chain's `sh -n` guard
(`hint_runs.rs`) proves the line parses; it runs no indexer.

### Ingest

From `scip.proto` (field names verbatim):

| SCIP | Our facts |
|---|---|
| `Document.relative_path`, `.language` | `file(F, Lang)` — `Lang` lowercased from the `Language` enum |
| `Occurrence` with `symbol_roles & Definition` | a definition site; `symbol` → `SymId` |
| `SymbolInformation.kind` | `Kind`, via the mapping table below |
| `SymbolInformation.display_name`, else the name of the symbol's final descriptor | `Name`. `scip-python` writes no `display_name` for a parameter, an attribute or a module; the descriptor grammar is mandatory and the name is written in it, so reading it is not a guess. A definition occurrence with no `SymbolInformation` at all is named the same way, from its own symbol string; it is never `""` (#47). The kind is read from the descriptor only for the **namespace** suffix, `/`, which the grammar gives exactly one meaning — see § A namespace descriptor is a `module`. No other suffix is read, and `.` is why: it is a field, a variable or a constant, and the suffix does not say which |
| symbol-string descriptor prefix, else `SymbolInformation.enclosing_symbol` | `parent(S, Owner)` — replaces tier A's row, see § Parent precedence |
| `SymbolInformation.documentation[]` | `def_doc` (if tier A did not supply one), **minus every element that is wholly one fenced code block** — see § Hover blocks are not doc comments |
| `SymbolInformation.signature_documentation.text` | `def_sig` (if tier A did not supply one) |
| `Occurrence` without `Definition` role | `scip_ref(S, F, L, C, From, Role)` |
| `symbol_roles` bits | `Role`: `WriteAccess`→`write`, `ReadAccess`→`read`, `Import`→`import`, `Test`→`test`, `Generated`→`generated`, `ForwardDefinition`→`forward`, empty bitset (`UnspecifiedSymbolRole`)→`unknown`. `Definition` is not a `Role`: that occurrence is a definition site (the row above) and never reaches `scip_ref` (#44) |
| `Relationship.is_implementation` | `scip_impl(S, T)` — `implements(S, T, "exact")` is the rule over it |
| `Relationship.is_type_definition` | `has_type(S, T, "exact")` |
| symbol string names a package **and no document defines it** | `extern(S, Manager, Pkg, Version)` |
| every ingested definition | `resolved(S)` |

`SymbolInformation.kind` has **80+ values**; we collapse them:

```
Function Macro                                   -> function / macro
Method AbstractMethod StaticMethod SingletonMethod
  TraitMethod ProtocolMethod PureVirtualMethod
  MethodSpecification TypeClassMethod Getter
  Setter Accessor MethodAlias                    -> method
Constructor                                      -> constructor
Class SingletonClass                             -> class
Struct                                           -> struct
Interface Protocol                               -> interface
Trait TypeClass Concept                          -> trait
Enum                                             -> enum
Union Type                                       -> type
Field Property StaticField StaticProperty
  StaticDataMember Key                           -> field
Constant EnumMember                              -> constant
Variable StaticVariable Value Object Instance    -> variable
Module Namespace Package PackageObject File      -> module
TypeAlias TypeFamily DataFamily AssociatedType   -> typealias
everything else                                  -> unknown
```

`Type` maps to `type`: SCIP's generic "a type" says nothing about being an
alias, and the code has mapped it so since the first commit (486c682); the
table read `typealias` until #126, and nothing pinned either. `Union` maps to
`type`, not to `enum`. An earlier draft collapsed the two, which
made a Rust `union` and a Rust `enum` produce identical rows — a smaller version
of the upstream `tags.scm` defect that `docs/plan.md` M2's kind-fidelity test
exists to catch, and the same class of extractor guess invariant 1 forbids. Both
tiers map it the same way so the anchor join never has to reconcile a kind.

### A namespace descriptor is a `module`

`SymbolInformation.kind` is the first source for `Kind`, and when it is
unspecified — `scip-typescript` 0.4.0 writes no `kind` at all — the fallback is
`unknown`, not a guess. One suffix is the exception: a symbol whose **final
descriptor is a namespace**, written `/`, is a `module`.

This is the same argument that licenses reading `Name` from the descriptor. The
descriptor grammar is mandatory, every indexer writes it, and `/` has one
meaning in it. The carve-out above — "the suffix does not say which" — is about
`.`, which covers a field, a variable and a constant at once. It was never an
argument against `/`, and treating it as one left every document's own module
symbol wearing `unknown`.

That mattered because of what reads `Kind`. `calls_at` refuses a caller whose
kind is `module` ([01-facts.md](01-facts.md) § Derived relations), and with the
document symbol typed `unknown` the refusal could not see the one definition it
exists to exclude (#75).

**`:` (Meta) is not read, and a Python module stays `unknown` (#129).**
`scip-python` 0.6.6 writes each file's own module symbol with a Meta suffix —
`kinds/__init__:` — so on the Python fixture 8 modules are `unknown` rows named
`__init__`, beside three real `__init__` methods. The obvious mapping, Meta →
`module`, has a counterexample in our own fixtures: `scip-typescript` writes a
type-literal scope as ``ui/`row.tsx`/Row().(props)typeLiteral0:``. `/` has one
meaning in the grammar and `:` has at least two, so mapping it would make one
indexer's convention outrank the grammar (invariant 1). Separate the two by
`Kind` (`method` vs `unknown`). A test pins both symbols as `unknown`, so
reading Meta is a deliberate spec change, not a drift.

### Hover blocks are not doc comments

`scip.proto` says `documentation` is a list of **Markdown** sections, and the
indexers do not agree on what goes in them:

| Indexer | `documentation[]` | `signature_documentation` |
|---|---|---|
| `scip-go` 0.2.7 | the doc comment, as prose | present |
| `rust-analyzer` 1.98.1 | the doc comment, as prose | present |
| `scip-python` 0.6.6 | `[0]` is a ```` ```python ```` hover block; `[1..]` the docstring, when there is one | **absent** |
| `scip-typescript` 0.4.0 | `[0]` is a ```` ```ts ```` hover block; `[1..]` the JSDoc, when there is one | **absent** |

An indexer that writes no `signature_documentation` renders the signature into
`documentation` instead, as a fenced code block. It does this for **every**
symbol, documented or not. Joining the whole list into `def_doc` therefore made
`def_doc` hold for almost everything, and `specs/01-facts.md`'s contract —
absent means undocumented — stopped being true the moment a SCIP index was
present. `undocumented_export(S) :- exported(S), !def_doc(S, _).` returned
nothing on a whole TypeScript package (#55). Where a real comment did exist, the
fence was glued to the front of it.

So: **an element of `documentation[]` that is wholly one fenced code block is a
signature, and is dropped.** The remaining elements join as before; if none
remain there is no `def_doc` row.

This is read, not guessed. "Is this element one fenced block, start to end" is a
question about its bytes, asked per element — the list is never flattened first,
so a doc comment that merely *contains* an example keeps it. The rule is uniform
across languages, which is the point: no `if lang == "typescript"` lives in the
ingest, and an indexer that later starts writing `signature_documentation` needs
no change here.

What it costs: a doc comment whose entire body is one code block and nothing
else is dropped. Such a comment is byte-identical to a rendered hover, so the
extractor cannot tell them apart, and invariant 1 says emit nothing rather than
guess. The fenced text is **not** rerouted — not into `def_sig`, whose contract
in `specs/01-facts.md` is a one-line declaration header (a `scip-python` hover
block is several lines and a `scip-typescript` one is prefixed `(method)`, so
reflowing either would be the transformation invariant 1 forbids), and not into
a relation of its own, which the surface budget in
`crates/codeintel/tests/surface.rs` has no room for (#42).

### Position normalization

SCIP ranges carry a `PositionEncoding` (`UTF8` / `UTF16` / `UTF32` code units
from line start), and `Document.position_encoding` may differ per document.
**Normalize on ingest** to our convention (1-based line, 0-based UTF-8 byte
column) using the document's `text` when present, or the file on disk otherwise.

If a document specifies UTF-16 and its source is unavailable, **skip that
document and report it** in `status` — do not emit approximate columns. A
half-byte-wrong column silently breaks every edit built on it.

**An unspecified encoding is not UTF-8.** `UnspecifiedPositionEncoding` is what
an indexer writes when it does not say, and not saying is not counting bytes:
`scip-python` 0.6.6 and `scip-typescript` 0.4.0 both declare nothing and count
UTF-16. Both place `x` in `s = "ééé"; x = 1` at column 11; its byte column is 14
(measured 2026-09-13, #21). Read as bytes, every occurrence after a non-ASCII
character on its line lands on the wrong byte, and a definition that then fails
to anchor comes back as a second, tier-B-only `def`.

So a declared encoding is transcoded as above, and an unspecified column is
kept only where every encoding agrees on it: **when the line's text before the
column is ASCII**, its UTF-8, UTF-16 and UTF-32 columns are the same number.
Otherwise the column is ambiguous. The occurrence is skipped and counted per
SCIP input as `ambiguous`, in the manifest, in the `index` summary and in
`status`. An ambiguous endpoint of an `enclosing_range` drops that range, not
the definition it belongs to.

There is no table of which tool counts what. The declared encoding is the only
statement an index makes about its columns, and a belief about a tool's
behaviour goes stale the release it changes. Telling ASCII from not needs the
text, so an unspecified document whose source is unavailable is skipped exactly
as a UTF-16 one is.

The same rule applies per occurrence, not only per document:

- A **negative line or column** is a malformed range. Skip the occurrence; do
  not clamp it to line 1 or column 0, which would place a real symbol at a real
  byte range that is not its own.
- A **line past the end of the transcoded source** means the file on disk no
  longer matches the index. Skip the occurrence rather than passing the
  code-unit column through untranscoded — for a UTF-16 document that is an
  approximate column, which the paragraph above forbids.

Prefer `single_line_range` / `multi_line_range` over the deprecated
`repeated int32 range` field; support both, since indexer versions vary.

### Deriving `From` for references

SCIP does not say which function a reference sits inside. We compute it with
**the same span sweep tier A uses for containment**:

1. Collect every definition's span in the file. Tier A supplies it for files it
   parsed. SCIP's `typed_enclosing_range` (its full-definition range) supplies
   it for every definition tier A has no span for — which is every definition
   in a file only tier B covers, and the ones tier A's grammar missed in a file
   it did parse.
2. Sort by `(start, -end)`, sweep with a stack, and for each reference position
   take the innermost enclosing definition. No enclosing definition → `From = F`.

Both tiers, one algorithm, one test suite. This is also why `calls` is a Datalog
rule rather than an extractor output: the extractor emits *where a reference is*,
and the rule decides what counts as a call.

#### Which span, when both tiers have one

**Tier A's span wins, per definition, keyed on symbol.** A SCIP enclosing range
is collected only for a definition tier A did not anchor. A symbol the anchor
join refused as a collision (§ The anchor join) contributes no span at all:
nothing adopted it and no `def` row is emitted for it, so it is not a
definition this index knows. The tier-A definition at that position keeps its
own symbol and its own span, and that span is collected as usual.

**The order is not a preference, it is a freshness argument.** Tier A parsed the
bytes on disk, this run. A SCIP range is a claim about the bytes the indexer saw,
which may be older — that is the whole content of `scip-stale`
([05-surface.md](05-surface.md) § Status taxonomy), and the same ordering the
project makes everywhere provenance is at stake. Preferring the older claim
where a fresher one exists would put a reference inside a definition that has
since moved, and report it as `exact`.

**What this does not say.** It is a rule about *definitions*, not about regions.
Two spans from the two models may overlap without either being a tier-A and
SCIP view of the same symbol, and the sweep settles those the way it settles
every other overlap: innermost wins, and two items with an identical span
resolve by input order (`crates/extract/src/sweep.rs`). There is no tier column
in the sweep and there should not be one.

This paragraph was absent until #80, and its absence read as a prohibition.
Step 1 named the two clean cases — tier A parsed the file, or it did not — and
said nothing about a SCIP-only definition *inside* a parsed file, which the code
has always used. The alternative reading was that such a definition should
contribute no span: one span model per file, and the sweep never compares two
rulers. It was rejected because it answers a span-*source* question with a
*file*-granularity switch. A definition tier A missed is a hole in the owner
map, and SCIP's range is the only thing that can fill it; discarding it in a
parsed file also discards it where tier A has no competing span at all, and
there is nothing to reconcile in that case.

**The evidence for that choice is thin, and saying so is part of the spec.**
It was measured on `tests/fixtures/typescript/` — one fixture, one indexer. There
the only SCIP-only definition in a tier-A-parsed file that contributes an owner
span is the document's own module symbol; every other SCIP definition carrying an
enclosing range is tier-A anchored, and parameters and locals carry none. So the
two readings differ there by exactly the symbol #75 removed from `calls`, and the
rejected reading would have cost nothing measurable. That is not a general result.
`scip-clang` and `scip-java` cover languages tier A does not parse at all.

**The measurement that would settle it**, for whoever has a quiet host and the
four indexers installed: per language, how many SCIP definitions in a
tier-A-*parsed* file carry a `typed_enclosing_range` and have no tier-A span.
Near zero everywhere and this is a paragraph about nothing, and step 1 can be
tightened to the simpler rule. Substantial in any language, and the gap-fill is
load-bearing and belongs in the done-when list for that language.

### Parent precedence

Both tiers can supply a parent and they disagree in real cases. `parent` carries
the **semantic owner**, resolved by a fixed order
([01-facts.md](01-facts.md) § `parent`):

1. Drop the last descriptor from the SCIP symbol string. `pkg/Store#Get().` →
   `pkg/Store#`. **If that names a definition this SCIP index actually has**, it
   is the parent. Preferred because the descriptor grammar is mandatory, so
   every indexer supplies it. The existence check is not optional: a Go method
   truncates to a type the index may not define, and a `parent` pointing at a
   symbol nothing defines is worse than no row at all. "Has" means **the join
   will emit a `def` for it**: the set is every symbol the index defines, less
   the ones the join refuses (§ The anchor join: defined in two documents, or
   two symbols at one position). A refused symbol keeps no `def` row under
   that name, so an owner check that counted it produced exactly the dangling
   `parent` the check exists to prevent. The set is computed once per ingest
   and once more after a merge, not per file.
2. Else `SymbolInformation.enclosing_symbol`, which is how SCIP `local` symbols
   get an owner — subject to the same existence check.
3. Else tier A's answer — a resolved `@owner` where the language captured one
   (§ `@owner`), else span nesting: the innermost enclosing span that is
   **itself a definition**, else the file. Subject to the same existence check
   as 1 and 2, for the same reason, and it is the rule that needs it most: it
   is the fallback everything else falls through to, so a hole here is not an
   edge case.

Rule 3's check is not redundant with the span sweep. The sweep nests *spans*,
and not every span is a definition — a Rust `impl` block encloses its methods
and is emitted as no `def` row. Taking the innermost enclosing *item* rather
than the innermost enclosing *definition* therefore produced
`parent(Get, "local src/store.rs Store#")`, naming a symbol with no `def` row
and no `parent` row of its own. `within/2` recurses through `parent`, so it
terminated one hop short of the file and silently omitted every method in an
`impl` block from any transitive containment query. Silent, because the query
*succeeded*: the status taxonomy can see a failed answer, not a short one. That
is the failure mode invariant 5 exists to prevent, arriving through a door
invariant 5 does not watch — which is why the check belongs here, in the
extractor, and not in a status code.

Tier B **replaces** tier A's row rather than adding one, so the
one-row-per-definition guarantee holds. Replacement happens after the anchor
join, when the tier-A symbol already carries its SCIP identity.

Descriptor truncation must be done on the parsed descriptor list, not by string
surgery — descriptor names can contain `#`, `.`, and `/` when backtick-quoted
(SCIP escapes them as `` `name with spaces` ``), so a naive `rsplit` on the
suffix character corrupts them.

### External symbols

"Package ≠ project package" is not directly checkable: SCIP never states which
packages a project *is*. It states, exactly, which symbols the project defines.
So `extern` is the complement — a symbol that names a package and that no
document in this index defines — and needs no guess.

### Local symbols

SCIP `local N` symbols are **document-scoped**. Rewrite them on ingest to
`local <relative_path> N` before interning. Skipping this silently cross-links
unrelated locals across every file in the repo.

---

## The anchor join

When both tiers cover a file, identity is unified so there is exactly one `def`
row per definition.

```
for each SCIP definition occurrence D in file F:
    find the tier-A definition whose def_name position == D's range start
    if found:  that def adopts D.symbol as its SymId; emit resolved(S)
    if absent: emit a tier-B-only def from SymbolInformation (no span/sig)
```

Matching on the **identifier position** (`def_name`), not the full span, is what
makes this robust: the two tiers disagree about whether a span includes
decorators, attributes, and doc comments, but they always agree on where the
name token starts.

Rewriting is done in the interner: the tier-A synthetic symbol string is mapped
to the SCIP symbol's atom id before any facts are written, so every tier-A fact
referencing it (`def_span`, `def_sig`, `parent`, `exported`, `ref.From`, ...)
lands on the resolved identity for free. Nothing is rewritten twice.

Ordering requirement: **tier A runs first, entirely, then tier B.** Tier B needs
tier A's `def_name` index to anchor against.

**A symbol the index defines in more than one document is not adopted.** One
`SymId` with two definitions violates [01-facts.md](01-facts.md) § `def` (one
row per definition) and breaks every rule built on it: `at/3` cross-multiplies
the files and lines, `innermost_at` answers from the other file, `impact_of`
unions unrelated call graphs, `dead_export` never fires. So ingest collects the
symbols defined in two or more documents, the join skips them, and tier B
emits no `def`, `resolved`, `implements` or `extern` for them: the definition
keeps its tier-A `local` symbol, and `status` reports how many were refused.
Their `scip_ref` rows stay — they are facts — and join to nothing. Rekeying
them by document, or matching by name instead, would be the extractor deciding
what the indexer did not (invariant 1); the fix belongs upstream.

**Two symbols defined at one identifier position are refused the same way.**
An anchor is keyed by the position of the name token, so a position can carry
one identity; a second symbol there is either the indexer describing one
declaration twice or a document two inputs both describe, and picking the
last one read made `--scip a --scip b` and `--scip b --scip a` adopt different
identities from the same files. Both symbols are refused, counted with the
collisions, and excluded from the set rule 1 checks against. The same symbol
occurring twice at one position is one definition and is not a conflict.

**A file the SCIP inputs did not see is not joined** ([04-storage.md](04-storage.md)
§ Manifest, `scip_hash`): tier B emits nothing for it, and it is extracted as
tier A alone. Its positions have moved, so anchoring against the old ones
adopts the wrong identity or none, and the fallback — a tier-B-only `def`
beside the tier-A one — is two `def` rows for one definition, with `From`
computed from new text at old positions. Nothing is inferred to bridge the gap:
the file's definitions are `local` and `status` says `scip-stale` until the
indexer reruns.

### Duplicate suppression

A call site visible to both tiers contributes a `name_ref` *and* a `scip_ref`,
so the derived `ref` relation carries two rows for it, `"name"` and `"exact"`.
**Keep both.** They are different facts. `calls(A,B)` deduplicates naturally
because it projects `Prov` away; `calls_exact` selects the precise one.

Collapsing them would destroy the ability to ask "what does tier A see that
tier B missed" — which is the diagnostic for a broken, partial, or stale SCIP
index, and is expressible as a query:

```prolog
?- name_ref(N, F, L, C, From), !scip_ref(_, F, L, C, _, _).
```

Rows here are either tier-A false positives or genuine tier-B gaps. Either way
the number should be small and stable; a jump means one of the tiers changed
behaviour.

---

## Validation

Per-language fixture repos under `tests/fixtures/<lang>/`, each with a
hand-written golden fact file.

1. **Golden facts.** `cargo test` extracts the fixture and diffs against
   `expected.facts` (sorted, text). Any extraction change shows up as a legible
   diff.
2. **Span exactness.** Re-parse the file, and for every `def` assert that
   `end_byte` is where a node ends, that the outermost node inside the span
   ending there is the definition and is not a parse error, that `start_byte` is
   that node's start or the start of the contiguous attributes, comments and
   keywords before it (the leading siblings `def_span` reaches back over, and
   nothing else), and that `def_name` is the one identifier node, inside the
   definition, whose text is `Name`. Catches off-by-one and encoding bugs, a
   span that swallows its neighbour (#12), and a name that merely *starts* the
   line — all invisible to a containment check, and invisible until someone
   applies an edit.

   **In context, not the slice alone.** Parsing `src[start_byte..end_byte]` by
   itself was the first wording, and it cannot be asserted: a method, a field, an
   enum variant and a TypeScript constructor are nodes only inside their parent,
   so on the four fixtures 39 of 167 definitions fail it with exact spans —
   counted as `root.has_error()` on the slice parsed alone: Rust 8, Go 11,
   Python 0, TypeScript 20 (#126). This read 59, by a criterion that was never
   recorded and so could not be re-run. That check is not run. **The kind is not asserted here** either: that a node is a
   `struct` and not a `type` is `Kind fidelity`'s job and the golden file's, and
   restating it from the same query would agree with the query by construction.
3. **Anchor coverage.** On a fixture with both tiers, assert the **exact set**
   of tier-A definitions that do *not* get `resolved(S)`, by `file:Name`. Any
   other definition failing to anchor is the join drifting, and the assertion
   names which.

   **A set, not a rate, at this size.** `index` still computes and prints the
   rate, and ≥ 95% remains the done-when for a language against a real tree
   ([plan.md](../docs/plan.md) M5). Over a 26-to-63-definition fixture a rate
   is arithmetic about the fixture: one miss is three to four points, so the
   gate sat at zero headroom on all three fixtures at once and failed on the
   next definition anyone added, whatever the join had done — and its failure
   pointed at regenerating a committed SCIP index, making an installed indexer
   a prerequisite for adding a fixture (#79). A named set is stable under
   fixture growth and strictly stronger: a rate tolerates any drift it can
   round away, where a set tolerates none. Measured: a one-line shift in
   `tests/fixtures/go/store/store.go` takes the set from 1 member to 10, and
   names all 10.

   A member is admissible only when the definition cannot anchor for a reason
   that is not the join's doing — the indexer declares no position encoding and
   the line is not ASCII before the column (#21), or the committed index holds
   no occurrence for the definition at all, whether because it has no document
   for the file or because the definition was appended after it was built.
   Anything else is a regression, and regenerating the index to make it pass
   buries it. This is the exemption shape item 4 rejects for *defects*: these
   are properties of the committed index, not entries on a list of things the
   join should have managed and did not.

   **Modules are exempt, and the exemption is pinned by count.** Tier A places
   module `conn` at the `mod conn;` declaration in `src/db/mod.rs`;
   rust-analyzer places it at line 1 of `src/db/conn.rs`. They are in different
   files, so a join on identifier position structurally cannot match them. This
   is a property of the two models, not a defect in the join, and the fix is not
   a looser join — it is either a second `parent`-shaped relation for module
   containment or an extractor that stops treating `mod x;` as a definition.
   Both are out of scope here and neither is urgent. Measured on
   `tests/fixtures/rust/`: 35/35 non-module definitions anchor, and 1 of 10
   modules does — the inline `mod detail { }` in `src/store.rs`; the other nine
   are `mod x;` declarations.
4. **Tier-A precision.** On a fixture with both tiers, **no** derived `"name"`
   `ref` may sit at a position an `"exact"` `ref` covers. Zero by construction,
   not zero by allowlist: the `name` rules carry `!scip_claim(...)`
   ([01-facts.md](01-facts.md) § Tier precedence), so tier A cannot resolve a
   name at a position tier B already answered.

   This previously said "or be listed in `known-imprecise.txt` with a reason",
   and the list had one entry — `self.entries.get(key)` inside `Store::get`,
   where the same-file rule binds `get` to the enclosing method while SCIP knows
   it is `HashMap::get`. Writing the defect down bounded it, but it also made it
   permanent: an accepted exception is not a bound, it is a decision to stop
   counting. The guard deletes the row instead, and the allowlist with it,
   because an exception list nothing populates reads as rigour while asserting
   nothing.
5. **Locality.** Every tier-A fact for file X must be reproducible by extracting
   file X *alone*, with no other file indexed. This is the mechanical guard on
   incremental soundness — it fails loudly the moment someone reintroduces a
   cross-file lookup into the extractor.
6. **Idempotence.** Index twice → byte-identical segments.
7. **Determinism across order.** Index with files shuffled → identical facts.
8. **Incremental equivalence.** Index the fixture; mutate one file; reindex
   incrementally. The resulting fact set must equal a full cold reindex, byte
   for byte. This is the test that would have caught the bug `name_ref` fixes.
9. **Module spelling.** No `import` row's `Module` may start or end with a
   source delimiter, in any language. The only guard about agreement *between*
   languages: a language that kept its quotes would answer
   `?- import(F, "fmt", _).` with zero rows on that language alone, and a
   per-language golden cannot see a disagreement it is not party to (#70).
10. **Coverage is reported, never inferred.** A fixture whose SCIP documents are
    relative to a subdirectory is indexed twice: once without `--scip-root`,
    asserting zero matched documents, a report that says so, and a suggestion
    naming the prefix; once with it, asserting the facts land. A case with two
    candidate prefixes asserts that nothing is suggested. The first case passing
    silently is the bug this validates against (#60).
