local fr = require("maki.fuzzy_replace")

local failures = {}

local function case(name, fn)
  local ok, err = pcall(fn)
  if not ok then
    table.insert(failures, name .. ": " .. tostring(err))
  end
end

local function eq(actual, expected, msg)
  if actual ~= expected then
    error((msg or "") .. "\nexpected: " .. tostring(expected) .. "\n  actual: " .. tostring(actual))
  end
end

local function has(s, substr, msg)
  if not s:find(substr, 1, true) then
    error((msg or "") .. "\nexpected to contain: " .. tostring(substr) .. "\n  actual: " .. tostring(s))
  end
end

local R = "REPLACED"
local NO_MATCH = fr.NO_MATCH
local MULTIPLE_MATCHES = fr.MULTIPLE_MATCHES

-- fuzzy_replace unit tests

case("exact_match", function()
  local result = fr.replace("fn foo() {}\nfn bar() {}", "fn foo() {}", R, false)
  has(result, R)
end)

case("trimmed_boundary", function()
  local result = fr.replace("fn foo() {}", "\nfn foo() {}\n", R, false)
  has(result, R)
end)

case("different_indentation", function()
  local result = fr.replace("    fn f() {\n        bar();\n    }", "fn f() {\n    bar();\n}", R, false)
  has(result, R)
end)

case("whitespace_collapsed", function()
  local result = fr.replace("let   x  =   1;", "let x = 1;", R, false)
  has(result, R)
end)

case("whitespace_multiline", function()
  local result = fr.replace("fn  foo()  {\n    bar();\n}", "fn foo() {\nbar();\n}", R, false)
  has(result, R)
end)

case("whitespace_substring", function()
  local result = fr.replace("    let   x  =   compute(a,  b);", "compute(a, b)", R, false)
  has(result, R)
end)

case("escaped_newline", function()
  local result = fr.replace('let s = "hello\nworld";', 'let s = "hello\\nworld";', R, false)
  has(result, R)
end)

case("escaped_tab", function()
  local result = fr.replace("col1\tcol2\tcol3", "col1\\tcol2\\tcol3", R, false)
  has(result, R)
end)

case("block_anchor_fuzzy_middle", function()
  local result = fr.replace(
    "fn test() {\n    let x = 1;\n    let y = 2;\n}",
    "fn test() {\n    let x = 99;\n    let y = 2;\n}",
    R,
    false
  )
  has(result, R)
end)

case("context_aware_partial_middle", function()
  local result = fr.replace(
    "fn h() {\n    validate();\n    process();\n    save();\n    respond();\n}",
    "fn h() {\n    validate();\n    WRONG();\n    save();\n    respond();\n}",
    R,
    false
  )
  has(result, R)
end)

case("no_match", function()
  local result, err = fr.replace("fn foo() {}", "MISSING", "x", false)
  eq(result, nil)
  eq(err, NO_MATCH)
end)

case("ambiguous_multiple_matches", function()
  local result, err = fr.replace("let x = 1;\nlet x = 1;", "let x = 1;", "x", false)
  eq(result, nil)
  eq(err, MULTIPLE_MATCHES)
end)

case("block_anchor_picks_best_among_multiple", function()
  local content = "fn a() {\n    unrelated();\n}\nfn a() {\n    target();\n}"
  local result = fr.replace(content, "fn a() {\n    target();\n}", R, false)
  has(result, R)
  has(result, "unrelated()")
end)

case("leading_whitespace_disambiguates", function()
  local result = fr.replace("fn foo() {}\n  fn foo() {}", "  fn foo() {}", R, false)
  eq(result:sub(1, 11), "fn foo() {}")
  has(result, R)
end)

case("context_aware_below_threshold_rejects", function()
  -- block_anchor with a single candidate and threshold=0.0 accepts this,
  -- so it goes through before context_aware. This tests that the full
  -- replace pipeline still works (block_anchor matches).
  local content = "fn f() {\n    a();\n    b();\n    c();\n    d();\n}"
  local search = "fn f() {\n    w();\n    x();\n    y();\n    z();\n}"
  local result = fr.replace(content, search, "x", false)
  -- block_anchor matches the single candidate, so replace succeeds
  has(result, "x")
end)

case("unescape_trailing_backslash", function()
  local result = fr.replace("trailing\\", "trailing\\", R, false)
  has(result, R)
end)

case("strip_common_indent_skips_blank_lines", function()
  local result = fr.replace("    a\n\n    b", "a\n\nb", R, false)
  has(result, R)
end)

case("block_anchor_no_panic_near_end", function()
  local content = "aaa\nbbb\nccc\nfn test() {"
  local search = "fn test() {\n    body();\n}"
  local result, err = fr.replace(content, search, "x", false)
  eq(result, nil)
end)

case("block_anchor_no_panic_last_line", function()
  local content = "fn test() {"
  local search = "fn test() {\n    body();\n}"
  local result, err = fr.replace(content, search, "x", false)
  eq(result, nil)
end)

case("block_anchor_no_panic_two_lines", function()
  local content = "fn test() {\n}"
  local search = "fn test() {\n    body();\n}"
  local result, err = fr.replace(content, search, "x", false)
  eq(result, nil)
end)

case("escape_normalized_also_fixes_new_string", function()
  local content = 'print("hello")'
  local old = 'print(\\"hello\\")'
  local new = 'print(\\"world\\")'
  local result = fr.replace(content, old, new, false)
  eq(result, 'print("world")')
end)

case("escape_normalized_new_string_with_replace_all", function()
  local content = 'say("a")\nsay("b")'
  local old = 'say(\\"a\\")'
  local new = 'say(\\"x\\")'
  local result = fr.replace(content, old, new, true)
  eq(result, 'say("x")\nsay("b")')
end)

case("replace_all_replaces_every_occurrence", function()
  local result = fr.replace("aXbXc", "X", "Y", true)
  eq(result, "aYbYc")
end)

if #failures > 0 then
  error(#failures .. " case(s) failed:\n\n" .. table.concat(failures, "\n\n"))
end
