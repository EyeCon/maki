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
- extra_body mechanics: `ProviderDef.extra_body` (BTreeMap<String, Value>) in
  providers.toml is merged into every inference body at
  `OpenAiCompatProvider::wire_body` (send path: `serde_json::to_vec(&wire_body(body))`).
  Auto-resolved in `OpenAiCompatProvider::new` for builtin openai-compat slugs
  (openrouter, zai, mistral, deepseek, tensorx, synthetic, regolo, opencode, xai,
  openai, aperture, ollama, llama-cpp). Empty-slug compat configs (opencode
  catalog) skip the providers.toml load by construction, so no auto-resolve
  there; custom providers get theirs via `with_extra_body` in `custom::create`
  (openai + openai-responses paths; custom anthropic/google ignore it). Flagged
  in `ignored_builtin_fields` for builtins whose inventory protocol isn't openai.
- Per-key strategies for combining with maki's computed fields
  (`ProviderDef.extra_body_policy`, BTreeMap<String, ExtraBodyPolicy>, added
  2026-09-05, change wlummqpn — replaces an earlier hardcoded `tools` append):
  - `replace` (default): config value wins wholesale. Strategy for keys maki
    never computes — OpenRouter `preset`, provider routing
    (`[openrouter.extra_body.provider]`, e.g. `ignore = ["relace/fp4"]`),
    scalar knobs, deprecated `plugins`.
  - `merge`: objects merge recursively, arrays concatenate, scalars replace;
    a computed-missing key is set. Strategy for extending computed
    arrays/objects — OpenRouter server tools
    (`[[openrouter.extra_body.tools]] type = "openrouter:web_search"` keeps
    maki's client tools and appends the server tool; verified via base_url
    capture: 18 client tools intact + server tool), `stream_options.include_usage`,
    responses-protocol `reasoning` overrides. Merge cannot shrink or reorder a
    computed array — only append; wholesale reshaping needs `replace`.
  - `remove`: deletes the computed field; a configured value under the key is
    ignored. Strategy for stripping computed fields (e.g. `stream_options`);
    works even with an empty `extra_body` (separate removal pass).
  - Unknown policy strings fail the TOML parse at startup (exit code 2).
- TOML shape: nested keys under `extra_body` need full dotted headers
  (`[openrouter.extra_body.provider]` after a `[[openrouter.extra_body.tools]]`
  array-of-tables lands at `extra_body.provider`, not inside the last tool
  element; a bare `[...tools]` table header after the array-of-tables is a TOML
  error).
- Verification recipe, easiest first: (1) `MAKI_LOG_WIRE=1 RUST_LOG=debug` logs
  the full serialized request body at the send sites into
  `~/.local/logs/maki/maki.log` (added 2026-09-05: `log_wire_body` in
  providers/mod.rs, wired at openai_compat chat_completions, openai responses,
  anthropic messages, google streamGenerate; body contains prompts — opt-in
  debug only). (2) `--print --verbose` / stream-json for logical turn-by-turn;
  session `.jsonl` files under `~/.local/state/maki/sessions/`. (3)
  `OPENROUTER_BASE_URL=http://127.0.0.1:PORT/v1` env override pointing at a
  local capture server that dumps POST bodies and answers with a minimal SSE
  stream — byte-exact wire proof incl. headers, needed only when the log is
  not enough (e.g. response-side behavior).
- Gotcha: maki-config's test binary does NOT link maki-providers, so `inventory`
  builtin-provider entries are empty there — `ignored_builtin_fields` tests that
  depend on `builtin_provider` must live in maki-providers tests.

## metaconfig.toml (config file redirects, added 2026-09-05, change qwvmrzzs)

- `maki-storage/src/metaconfig.rs`: discovered at `<cwd>/.maki/metaconfig.toml` first,
  then config search dirs; first found wins; read once per process (OnceLock, like
  paths STRATEGY). Format: `[files]` table, keys are config names (`providers.toml`,
  `permissions.toml`, `mcp.toml`, `init.lua`, `.env`, `providers/`, `commands/`,
  `themes/`), values are paths (relative resolve against the metaconfig file's own
  dir, `~` expands).
- Semantics: value naming an existing file replaces the search for that name
  entirely; a directory (or a not-yet-existing path, so typos fail soft) searches
  `<dir>/<name>` before the regular dirs and falls back to them.
- Wiring: `paths::find_config_path` delegates to `metaconfig::find` → providers.toml,
  mcp.toml, .env, and the dynamic `providers/` dir redirect for free. Global
  `permissions.toml` and `init.lua` iterate `metaconfig::candidates`; `commands/` and
  `themes/` use `metaconfig::dir_override` (scan callers: override scanned last so it
  wins last-wins scans). Project files (`.maki/init.lua` etc.) keep fixed locations.
- Precedence change: global permissions.toml is now first-existing-wins across search
  dirs (was last-wins) so override > legacy > xdg is coherent everywhere.
- Test pattern: pure cores `metaconfig_path`, `parse`, `candidates_with`,
  `dir_override_with` take cwd/dirs/entries; never touch the cached OnceLock in tests.
- e2e probe trick: a malformed redirected providers.toml hard-exits printing the exact
  path read (`maki models` exits 2) — decisive proof of which file was loaded; custom
  provider + missing `api_key_env` error is the positive-path probe.

## Environment / testing

- `nextest`, `just`, `stylua`, `nix` not installed in this env (as of 2026-08-28); use
  `cargo test`, read justfile recipes directly. CI runs `stylua --check plugins/`.
- maki-providers model tests (`model::tests::discovered_*`, `catalog::tests`) are flaky
  — they race on globals (`set_known_models`, `SHARED_CATALOG` OnceLock in
  `catalog.rs` whose first init reads the real user config + on-disk cache, racing
  `seed_catalog_for_tests`). Confirmed failing on pristine main (2026-09-04):
  2 fail parallel, 1 single-threaded (`catalog::…::free_opencode_model_is_free`);
  `model::tests::discovered_pricing_decides_free::priced_is_not_free` fails even solo
  on this machine. Unrelated to feature work.
- More pre-existing flakes, both confirmed failing on pristine main (2026-09-05):
  maki-lua `pack::tests::update_review_*` / `applied_update_records_*` (varying
  failures per run, pass solo; "locked by another Maki process" staleness) and
  maki-ui `components::messages::tests::{live_snapshot_uses_panel_generation,
  theme_switch_repaints_highlighted_code}` (read the global theme GENERATION counter,
  bumped by other tests calling `theme::set`). Unrelated to feature work.
- `_ignored/nim-probe` is a scratch cargo project that dumps Nim tree-sitter ASTs
  (`cargo run -- <file.nim>`); handy for extractor iteration.

## Conventions confirmed

- Plugin specs: add `tests/lang/<lang>.lua` case file + `require` line in
  `plugins/index/tests/spec.lua` (alphabetized); runs as one Rust test per plugin via
  `maki-lua/tests/spec.rs` include_str!.
- Bundled plugins are compiled into the binary → `cargo build` required after lua edits.
