return function(U)
  local get_text = U.get_text
  local find_child = U.find_child
  local new_entry = U.new_entry
  local new_import_entry = U.new_import_entry
  local compact_ws = U.compact_ws
  local line_start = U.line_start
  local line_end = U.line_end
  local SECTION = U.SECTION
  local CHILD_BRIEF = U.CHILD_BRIEF
  local extract_fields_truncated = U.extract_fields_truncated

  local FN_KEYWORDS = {
    proc_declaration = "proc",
    func_declaration = "func",
    method_declaration = "method",
    iterator_declaration = "iterator",
    template_declaration = "template",
    macro_declaration = "macro",
    converter_declaration = "converter",
  }

  local SECTION_KEYWORDS = {
    const_section = "const",
    var_section = "var",
    let_section = "let",
  }

  local COMMENT_KINDS = {
    comment = true,
    documentation_comment = true,
    block_comment = true,
    block_documentation_comment = true,
  }

  -- Doc comments are extras that attach inside the previously-parsed node,
  -- so the raw end line would swallow the next item's docs. Walk the
  -- rightmost child chain, skipping trailing comments, to find the real end.
  local function effective_end_line(node)
    local count = node:child_count()
    if count == 0 then
      return line_end(node)
    end
    local last = node:child(count - 1)
    if COMMENT_KINDS[last:type()] then
      if count >= 2 then
        return effective_end_line(node:child(count - 2))
      end
      return line_start(last) - 1
    end
    return effective_end_line(last)
  end

  -- Splits on commas outside bracket groups, so `a/[x, y], b` stays intact.
  local function split_modules(text)
    local items = {}
    local depth = 0
    local current = ""
    for ch in text:gmatch(".") do
      if ch == "[" then
        depth = depth + 1
        current = current .. ch
      elseif ch == "]" then
        depth = depth - 1
        current = current .. ch
      elseif ch == "," and depth == 0 then
        items[#items + 1] = current
        current = ""
      else
        current = current .. ch
      end
    end
    items[#items + 1] = current
    return items
  end

  local function split_segments(text)
    local segments = {}
    for part in text:gmatch("[^/%s]+") do
      segments[#segments + 1] = part
    end
    return segments
  end

  -- `std/strutils` yields {"std","strutils"}; `std/[os, strformat]` fans out
  -- into one path per bracketed name sharing the "std" prefix.
  local function module_paths(text, paths)
    local prefix, bracket = text:match("^(.-)/?%[(.-)%]$")
    if prefix then
      local base = split_segments(prefix)
      for name in bracket:gmatch("[^,]+") do
        local path = {}
        for i, seg in ipairs(base) do
          path[i] = seg
        end
        path[#path + 1] = name:match("^%s*(.-)%s*$")
        paths[#paths + 1] = path
      end
    else
      local path = split_segments(text)
      if #path > 0 then
        paths[#paths + 1] = path
      end
    end
  end

  local function strip_clauses(text)
    text = text:gsub("%s+except.*$", "")
    text = text:gsub("%s+as%s+%S+$", "")
    return text
  end

  local function extract_import(node, source, keyword)
    local raw = get_text(node, source)
    local cleaned = raw:match("^%a+%s+(.+)$") or raw
    local paths = {}
    for _, item in ipairs(split_modules(strip_clauses(cleaned))) do
      module_paths(item:match("^%s*(.-)%s*$"), paths)
    end
    return new_import_entry(node, paths, keyword)
  end

  local function extract_from_import(node, source)
    local raw = get_text(node, source)
    local cleaned = raw:match("^from%s+(.+)$") or raw
    local base, names = cleaned:match("^(.-)%s+import%s+(.+)$")
    local paths = {}
    if base then
      local base_segments = split_segments(strip_clauses(base))
      for name in names:gmatch("[^,]+") do
        local path = {}
        for i, seg in ipairs(base_segments) do
          path[i] = seg
        end
        path[#path + 1] = name:match("^%s*(.-)%s*$")
        paths[#paths + 1] = path
      end
    else
      module_paths(strip_clauses(cleaned), paths)
    end
    return new_import_entry(node, paths)
  end

  local function declaration_names(list_node, source)
    local names = {}
    for _, child in ipairs(list_node:children()) do
      local kind = child:type()
      if kind == "symbol_declaration" then
        local name_node = child:field("name")[1]
        if name_node then
          names[#names + 1] = get_text(name_node, source)
        end
      elseif kind == "tuple_deconstruct_declaration" then
        names[#names + 1] = compact_ws(get_text(child, source))
      end
    end
    return names
  end

  local function extract_variable(node, source, keyword)
    local list = find_child(node, "symbol_declaration_list")
    if not list then
      return nil
    end
    local names = table.concat(declaration_names(list, source), ", ")
    if names == "" then
      return nil
    end
    local type_node = node:field("type")[1]
    local type_str = type_node and (": " .. compact_ws(get_text(type_node, source))) or ""
    local entry = new_entry(SECTION.Constant, node, keyword .. " " .. names .. type_str)
    entry.line_end = effective_end_line(node)
    return entry
  end

  local function type_name(decl, source)
    local sym = find_child(decl, "type_symbol_declaration")
    if not sym then
      return ""
    end
    local name_node = sym:field("name")[1]
    local name = name_node and get_text(name_node, source) or ""
    local generics = find_child(sym, "generic_parameter_list")
    if generics then
      name = name .. get_text(generics, source)
    end
    return name
  end

  local function field_text(field, source)
    local list = find_child(field, "symbol_declaration_list")
    if not list then
      return "_"
    end
    local names = table.concat(declaration_names(list, source), ", ")
    local type_node = field:field("type")[1]
    local type_str = type_node and (": " .. compact_ws(get_text(type_node, source))) or ""
    return names .. type_str
  end

  local function enum_variant_names(node, source)
    local names = {}
    for _, child in ipairs(node:children()) do
      if child:type() == "enum_field_declaration" then
        local sym = find_child(child, "symbol_declaration")
        local name_node = sym and sym:field("name")[1]
        names[#names + 1] = name_node and get_text(name_node, source) or "_"
      end
    end
    return names
  end

  local function definition_entry(node, source, name)
    local kind = node:type()
    if kind == "object_declaration" then
      local label = "object " .. name
      local inherits = node:field("inherits")[1]
      if inherits then
        label = label .. " of " .. compact_ws(get_text(inherits, source))
      end
      local entry = new_entry(SECTION.Type, node, label)
      local body = find_child(node, "field_declaration_list")
      if body then
        entry.children = extract_fields_truncated(body, source, "field_declaration", field_text)
      end
      return entry
    elseif kind == "enum_declaration" then
      local entry = new_entry(SECTION.Type, node, "enum " .. name)
      entry.children = enum_variant_names(node, source)
      entry.child_kind = CHILD_BRIEF
      return entry
    elseif kind == "tuple_type" then
      local entry = new_entry(SECTION.Type, node, "tuple " .. name)
      local body = find_child(node, "field_declaration_list")
      if body then
        entry.children = extract_fields_truncated(body, source, "field_declaration", field_text)
      end
      return entry
    elseif kind == "concept_declaration" then
      return new_entry(SECTION.Type, node, "concept " .. name)
    elseif kind == "distinct_type" then
      return new_entry(SECTION.Type, node, "distinct " .. name)
    elseif kind == "ref_type" or kind == "pointer_type" then
      local prefix = kind == "ref_type" and "ref" or "ptr"
      for _, inner in ipairs(node:children()) do
        local entry = definition_entry(inner, source, name)
        if entry then
          entry.text = prefix .. " " .. entry.text
          return entry
        end
      end
      return nil
    elseif kind == "type_expression" then
      local tuple = find_child(node, "tuple_type")
      if tuple then
        return definition_entry(tuple, source, name)
      end
      return new_entry(SECTION.Type, node, "type " .. name .. " = " .. compact_ws(get_text(node, source)))
    elseif kind == "call" then
      return new_entry(SECTION.Type, node, "type " .. name .. " = " .. compact_ws(get_text(node, source)))
    end
    return nil
  end

  local function extract_type(decl, source)
    local name = type_name(decl, source)
    if name == "" then
      return nil
    end
    for _, child in ipairs(decl:children()) do
      local entry = definition_entry(child, source, name)
      if entry then
        entry.line_start = line_start(decl)
        entry.line_end = effective_end_line(decl)
        return entry
      end
    end
    return nil
  end

  local function extract_function(node, source, keyword)
    local name_node = node:field("name")[1]
    if not name_node then
      return nil
    end
    local name = get_text(name_node, source)
    local generics = node:field("generic_parameters")[1]
    local generic_text = generics and get_text(generics, source) or ""
    local params_node = node:field("parameters")[1]
    local params = params_node and compact_ws(get_text(params_node, source)) or "()"
    local ret_node = node:field("return_type")[1]
    local ret = ret_node and (": " .. compact_ws(get_text(ret_node, source))) or ""
    local entry = new_entry(SECTION.Function, node, keyword .. " " .. name .. generic_text .. params .. ret)
    entry.line_end = effective_end_line(node)
    return entry
  end

  local function attr_texts(attrs, source)
    local texts = {}
    for _, a in ipairs(attrs) do
      local text = compact_ws(get_text(a, source))
      if not text:find("pop", 1, true) then
        texts[#texts + 1] = text
      end
    end
    return texts
  end

  local function apply_attrs(entries, attrs, source)
    if #attrs > 0 then
      local texts = attr_texts(attrs, source)
      for _, entry in ipairs(entries) do
        entry.attrs = texts
      end
    end
    return entries
  end

  local function section_entries(node, source, attrs)
    local keyword = SECTION_KEYWORDS[node:type()]
    local entries = {}
    for _, child in ipairs(node:children()) do
      if child:type() == "variable_declaration" then
        local entry = extract_variable(child, source, keyword)
        if entry then
          entries[#entries + 1] = entry
        end
      end
    end
    return apply_attrs(entries, attrs, source)
  end

  return {
    import_separator = "/",

    is_doc_comment = function(node, _source)
      local kind = node:type()
      return kind == "documentation_comment" or kind == "block_documentation_comment"
    end,

    is_module_doc = function(node, _source)
      local kind = node:type()
      return kind == "documentation_comment" or kind == "block_documentation_comment"
    end,

    is_attr = function(node, _source)
      return node:type() == "pragma_statement"
    end,

    is_test_node = function(node, source, _attrs)
      if node:type() ~= "call" then
        return false
      end
      local fn_node = node:field("function")[1]
      return fn_node ~= nil and get_text(fn_node, source) == "test"
    end,

    extract_nodes = function(node, source, attrs)
      local kind = node:type()
      local keyword = FN_KEYWORDS[kind]
      if keyword then
        local entry = extract_function(node, source, keyword)
        return entry and apply_attrs({ entry }, attrs, source) or {}
      elseif kind == "import_statement" then
        return { extract_import(node, source, nil) }
      elseif kind == "import_from_statement" then
        return { extract_from_import(node, source) }
      elseif kind == "include_statement" then
        return { extract_import(node, source, "include") }
      elseif kind == "export_statement" then
        return { extract_import(node, source, "export") }
      elseif SECTION_KEYWORDS[kind] then
        return section_entries(node, source, attrs)
      elseif kind == "type_section" then
        local entries = {}
        for _, child in ipairs(node:children()) do
          if child:type() == "type_declaration" then
            local entry = extract_type(child, source)
            if entry then
              entries[#entries + 1] = entry
            end
          end
        end
        return apply_attrs(entries, attrs, source)
      end
      return {}
    end,
  }
end
