# Hiraku language tooling

The language server and formatter use `hiraku_script::cst::SyntaxTree`, which
shares the compiler lexer and parser. The VM, engine and Bevy are not dependencies
of this crate. JSON is used only for the LSP wire protocol.

## Run

From the Hiraku Cargo workspace:

```sh
cargo build -p hiraku-lsp
./target/debug/hiraku-lsp
```

Configure your editor's LSP client to launch the `hiraku-lsp` binary over stdio
for `.hks` files (language ID `hks`). Stdout contains only Content-Length-framed
JSON-RPC. Logging goes to stderr. `lsp-server` owns stdio framing and IO threads;
Hiraku no longer maintains a separate transport parser. There is no network
listener or project-file mutation. Malformed protocol frames terminate the
stdio session; malformed HKS only produces document diagnostics.

Hiraku Editor embeds the typed `analysis` service instead of launching a child
process and roundtripping through JSON. In the Code tab, debounced background
syntax diagnostics include line/column positions; **Format Document** explicitly
formats the current in-memory draft and updates the graph via normal text edits.
Only one worker and one coalesced pending snapshot are kept; results are checked
against the document/revision before applying. Switching files or continuing to
type cannot apply stale formatting. Invalid syntax never produces a partial edit.

Current capabilities:

- initialize / shutdown / exit;
- open, close, full replacement and incremental UTF-16 document updates;
- versioned syntax diagnostics and syntax warnings;
- whole-document formatting;
- top-level document symbols;
- multiline delimiter folding, including incomplete documents.

Edits within a notification are applied transactionally; invalid UTF-16 positions
and stale versions do not corrupt the buffer. Requests operate on open buffers,
not stale disk contents. This first server reparses changed documents completely.
It does not yet expose semantic diagnostics, completion, hover, rename or
cross-module definition lookup. Engine-native signatures require a future
embedding-provided project/schema interface, not a dependency on Bevy.
The existing compiler's `project::compile_project_with_policy` already collects
module exports before checking bodies. Semantic tooling should use that project
context with authenticated host signatures/preludes, rather than inventing a
parallel checker or reporting every engine-native name as unknown.

## Formatter

```sh
cargo run -p hiraku-cli -- fmt path/to/story.hks
cargo run -p hiraku-cli -- fmt path/to/story.hks --check
cargo run -p hiraku-cli -- fmt path/to/story.hks --write
```

Default output is stdout; only `--write` modifies the specified file. `--check`
fails when formatting differs. `--indent-width 2` changes indentation (default 4).
LSP formatting additionally honors `insertSpaces: false` for tab indentation.

This is a conservative first formatter: it normalizes indentation, existing
horizontal whitespace and trailing whitespace, and adds a final newline. It
preserves statement line breaks, comments/block markers, string contents and
compound-token spelling, including template strings and CRLF. Invalid syntax
produces diagnostics and no edits. Operator spacing, line wrapping, comment
reflow and HSON-specific formatting are intentionally deferred until the CST
has complete grammar nodes; whitespace must not silently change HKS semantics.

## CST boundary

`SyntaxTree` stores an `Arc<str>`, byte-spanned raw tokens, structural nodes and
parse diagnostics. Every token (including trivia and malformed input) occurs
exactly once in the tree. Valid documents have top-level statement nodes;
parenthesis, bracket and brace groups remain available on incomplete edits.
`ast` is the canonical parser result, not a separate semantic grammar.

This initial structural CST is not a complete expression-level green tree and
does not replace AST/HIR or perform incremental reparsing. The next extension
should emit finer syntax events from the canonical parser, rather than create
an independent editor grammar. `source_text::LineIndex` provides checked UTF-8 /
UTF-16 conversion shared with embedders.

Protocol reference: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
