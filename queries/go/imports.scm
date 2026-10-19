; Go imports. `@module` is the import path literal; the extractor reads its
; CONTENT, the span between the quote tokens, so the row says `fmt` and not
; `"fmt"` — the same spelling every other language uses (01-facts.md
; § `import`). Nothing is resolved to a path. `@alias` is the local
; binding, which in Go is also `_` for an import-for-effect and `.` for a dot
; import; both are recorded verbatim rather than normalized away, because
; "imported only for its side effects" is a thing a conformance rule wants to
; ask about.
;
; One pattern, not two. An optional captured child keeps the aliased and plain
; forms in the same match, where two patterns would emit two `import` rows for
; every aliased import.

(import_spec
  name: [
    (package_identifier)
    (blank_identifier)
    (dot)
  ]? @alias
  path: (_) @module) @import
