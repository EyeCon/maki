# Memories

## Tree-sitter grammars

- Workspace uses `tree-sitter` 0.25 (downgraded from 0.26 for Nim): the `tree-sitter`
  crate has `links = "tree-sitter"`, so only ONE version can exist in the graph. The
  alaviss Nim grammar (git-only, rev-pinned `ac72ba30d16edf0be021588a9301ede4accd6cf4`)
  pins `tree-sitter ~0.25`. Don't re-bump tree-sitter without checking grammar deps.
- crates.io `tree-sitter-nim` 0.1.0 is an unrelated toy grammar (117-line grammar.js) —
  never use it. Real Nim grammar: github.com/alaviss/tree-sitter-nim (MPL-2.0).
- flake.nix `gitDepHashes`: tree-sitter-nim entry has hash `""` — run `nix build`, paste
  the real hash from the mismatch error (nix wasn't available when added).

## Nim grammar quirks (plugins/index/lang/nim.lua)

- `##` doc comments are extras that attach INSIDE the previously-parsed node's child
  chain, inflating that node's end line; they are NOT siblings of the next item, so the
  framework's doc-attach-forward rarely fires. `effective_end_line` walks the rightmost
  child chain skipping trailing comments to fix ranges.
- `test "name":` parses as a `call` node with `function` text `"test"` (no dedicated node).
- Import text is parsed textually (grammar wraps paths in expression_lists/infix
  expressions); except/alias clauses are stripped, bracket lists `std/[os, strfmt]` fan
  out via `module_paths` + depth-aware `split_modules`.

## OpenRouter presets / extra_body

- OpenRouter presets: 3 reference styles — `"model": "@preset/slug"`, separate
  `"preset": "slug"` body field, `"model": "model@preset/slug"`. Style 1/3 work with
  zero maki code (`openrouter/@preset/slug`): `build_body` sends `model.id` verbatim
  and unknown ids for known provider slugs fall back to manifest defaults (zero
  pricing → $0 cost display, 200k context). Custom `ModelDef`s under builtin slugs
  are ignored (`ignored_builtin_fields`), so those fallbacks can't be fixed via
  providers.toml.
- Style 2 implemented (2026-08-31, change ztzslvur): `ProviderDef.extra_body`
  (BTreeMap<String, Value>) in providers.toml, merged into every inference body at
  `OpenAiCompatProvider::wire_body` (last-wins: config beats maki's computed fields).
  Auto-resolved in `OpenAiCompatProvider::new` for builtin slugs (openrouter, zai,
  mistral, deepseek, tensorx, synthetic, local, opencode/catalog, xai/openai
  platforms); custom providers get it via `with_extra_body` in `custom::create`
  (openai + openai-responses paths; custom anthropic/google ignore it). Flagged in
  `ignored_builtin_fields` for builtins whose inventory protocol isn't openai.
- Gotcha: maki-config's test binary does NOT link maki-providers, so `inventory`
  builtin-provider entries are empty there — `ignored_builtin_fields` tests that
  depend on `builtin_provider` must live in maki-providers tests.

## Environment / testing

- `nextest`, `just`, `stylua`, `nix` not installed in this env (as of 2026-08-28); use
  `cargo test`, read justfile recipes directly. CI runs `stylua --check plugins/`.
- maki-providers model tests (`model::tests::discovered_*`, `catalog::tests`) are flaky
  under parallel execution — they mutate a global model registry (`set_known_models`).
  Confirmed failing on pristine main; unrelated to feature work. Pass solo.
- `_ignored/nim-probe` is a scratch cargo project that dumps Nim tree-sitter ASTs
  (`cargo run -- <file.nim>`); handy for extractor iteration.

## Conventions confirmed

- Plugin specs: add `tests/lang/<lang>.lua` case file + `require` line in
  `plugins/index/tests/spec.lua` (alphabetized); runs as one Rust test per plugin via
  `maki-lua/tests/spec.rs` include_str!.
- Bundled plugins are compiled into the binary → `cargo build` required after lua edits.
