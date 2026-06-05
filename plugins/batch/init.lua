local MAX_BATCH_SIZE = 25
local CHILD_SEP = "__"
local CHILD_INDENT = "  "
local CHILD_BODY_INDENT = "    "
local TOOL_SEPARATOR = "  ──────────────────"

local ToolView = require("maki.tool_view")

local description = [[Executes multiple independent tool calls concurrently to reduce round-trips.

ALWAYS USE THE BATCH TOOL WHEN YOU HAVE MULTIPLE INDEPENDENT TOOL CALLS. This dramatically improves performance.

Rules:
- 1-25 tool calls per batch
- All calls run in parallel; order NOT guaranteed
- Partial failures do not stop other calls
- Do NOT nest batch inside batch
- Do NOT use for dependent operations or when filtering results (use code_execution)]]

local function resolve_tool_name(raw)
  return raw.tool or raw.name
end

local function normalize_entry(raw)
  local name = resolve_tool_name(raw)
  local flat = {}
  for k, v in pairs(raw) do
    if k ~= "tool" and k ~= "name" and k ~= "parameters" then
      flat[k] = v
    end
  end
  if raw.parameters then
    if next(flat) then
      local merged = {}
      if type(raw.parameters) == "table" then
        for k, v in pairs(raw.parameters) do
          merged[k] = v
        end
      end
      for k, v in pairs(flat) do
        merged[k] = v
      end
      return { tool = name, parameters = merged }
    end
    return { tool = name, parameters = raw.parameters }
  end
  return { tool = name, parameters = flat }
end

local function child_header_line(tool_name, summary, annotation, status)
  local indicator_style, indicator_text
  if status == "running" then
    indicator_style = "spinner"
    indicator_text = "◐ "
  elseif status == "error" then
    indicator_style = "tool_error"
    indicator_text = "● "
  else
    indicator_style = "tool_success"
    indicator_text = "● "
  end
  local line = {
    { CHILD_INDENT .. indicator_text, indicator_style },
    { tool_name .. "> ", "tool_prefix" },
  }
  if summary and summary ~= "" then
    line[#line + 1] = { summary }
  end
  if annotation and annotation ~= "" then
    line[#line + 1] = { " (" .. annotation .. ")", "dim" }
  end
  return line
end

local function count_body_lines(r)
  if r and r.body then
    return r.body:len()
  end
  if r and r.output then
    local n = 0
    for _ in r.output:gmatch("\n") do
      n = n + 1
    end
    if #r.output > 0 then
      n = n + 1
    end
    return n
  end
  return 0
end

local function batch_header_line(total, status)
  local indicator_style, indicator_text
  if status == "running" then
    indicator_style = "spinner"
    indicator_text = "◐ "
  elseif status == "error" then
    indicator_style = "tool_error"
    indicator_text = "● "
  else
    indicator_style = "tool_success"
    indicator_text = "● "
  end
  return {
    { indicator_text, indicator_style },
    { "batch> ", "tool_prefix" },
    { total .. " tools" },
  }
end

local function child_status(r)
  if not r then
    return "running"
  end
  if r.is_error then
    return "error"
  end
  return "success"
end

local function get_tool_output_limits(ctx, tool_name)
  local tol = ctx and ctx:tool_output_lines() or nil
  if not tol then
    return 5
  end
  return tol[tool_name] or tol.other or 5
end

-- Each child renders into one stable slot buf, embedded into the parent
-- once. Embeds are live references, so child writes (streaming output,
-- click toggles, async highlights) show up in the parent without any
-- recomposing. Clicks route to the innermost buf with a handler: a child
-- that handles its own clicks behaves exactly as it does standalone, the
-- slot handler only sees rows the slot itself owns (e.g. the header line).
local function update_slot(slot)
  local tmp = maki.ui.buf()
  local r = slot.result

  if slot.full_view and r and r.body and r.body:len() > 0 then
    tmp:embed(r.body, CHILD_INDENT)
    slot.buf:assign(tmp)
    return
  end

  local body_lines = count_body_lines(r)
  local ann = body_lines > 0 and (body_lines .. " lines") or nil
  if r and r.rendered and r.rendered.annotation then
    ann = r.rendered.annotation
  end
  tmp:line(child_header_line(slot.tool, r and r.summary or "", ann, child_status(r)))

  if r and r.rendered then
    tmp:embed(r.rendered.body, CHILD_BODY_INDENT)
    if slot.view then
      tmp:embed(slot.view.buf, CHILD_BODY_INDENT)
    end
  elseif r and r.body and r.body:len() > 0 then
    tmp:embed(r.body, CHILD_BODY_INDENT)
  elseif slot.view then
    tmp:embed(slot.view.buf, CHILD_BODY_INDENT)
  elseif r and r.output and r.output ~= "" then
    for line in r.output:gmatch("[^\n]+") do
      tmp:line({ { CHILD_BODY_INDENT .. line, r.is_error and "error" or nil } })
    end
  end
  slot.buf:assign(tmp)
end

maki.api.register_tool({
  name = "batch",
  description = description,
  full_view = true,
  audiences = { "main", "research_sub", "general_sub" },
  schema = {
    type = "object",
    required = { "tool_calls" },
    properties = {
      tool_calls = {
        type = "array",
        description = "Array of tool calls to execute in parallel",
        items = {
          description = "Tool invocation: { tool: string, parameters: object } or flat { tool: string, ...params }",
        },
      },
    },
  },

  header = function(input)
    local n = input.tool_calls and #input.tool_calls or 0
    return n .. " tools"
  end,

  restore = function(_input, output, is_error, ctx)
    local buf = maki.ui.buf()
    local sections = {}
    local current_tool, current_lines

    for line in (output .. "\n"):gmatch("([^\n]*)\n") do
      local tool = line:match("^## (.+)$")
      if tool then
        if current_tool then
          sections[#sections + 1] = { tool = current_tool, lines = current_lines }
        end
        current_tool = tool
        current_lines = {}
      elseif current_tool then
        current_lines[#current_lines + 1] = line
      end
    end
    if current_tool then
      sections[#sections + 1] = { tool = current_tool, lines = current_lines }
    end

    buf:line(batch_header_line(#sections, is_error and "error" or "success"))

    for idx, sec in ipairs(sections) do
      if idx > 1 then
        buf:line({})
        buf:line({ { TOOL_SEPARATOR, "dim" } })
        buf:line({})
      end

      while #sec.lines > 0 and sec.lines[#sec.lines] == "" do
        sec.lines[#sec.lines] = nil
      end
      local sec_err = #sec.lines > 0 and sec.lines[1]:match("^%[ERROR%]") ~= nil
      local ann = #sec.lines > 0 and (#sec.lines .. " lines") or nil

      local slot = maki.ui.buf()
      slot:line(child_header_line(sec.tool, "", ann, sec_err and "error" or "success"))
      if #sec.lines > 0 then
        local view = ToolView.new(maki.ui.buf(), {
          max_lines = get_tool_output_limits(ctx, sec.tool),
          keep = "head",
        })
        for _, l in ipairs(sec.lines) do
          view:append({ { l, sec_err and "error" or nil } })
        end
        view:finish()
        slot:embed(view.buf, CHILD_BODY_INDENT)
        slot:on("click", function()
          view:toggle()
        end)
      end
      buf:embed(slot)
    end

    return buf
  end,

  handler = function(input, ctx)
    local raw_calls = input.tool_calls or {}
    if #raw_calls == 0 then
      return { llm_output = "provide at least one tool call", is_error = true }
    end

    local batch_id = ctx:tool_use_id() or ""
    local active_count = math.min(#raw_calls, MAX_BATCH_SIZE)

    local calls = {}
    for i = 1, #raw_calls do
      calls[i] = normalize_entry(raw_calls[i])
      if not calls[i].tool then
        return { llm_output = "each tool_calls entry must have a 'tool' field", is_error = true }
      end
    end

    local buf = maki.ui.buf()
    local slots = {}
    for i = 1, active_count do
      slots[i] = {
        buf = maki.ui.buf(),
        tool = calls[i].tool,
        full_view = maki.api.tool_full_view(calls[i].tool),
      }
    end

    local function compose(status)
      local tmp = maki.ui.buf()
      tmp:line(batch_header_line(#raw_calls, status))
      for i = 1, active_count do
        if i > 1 then
          tmp:line({})
          tmp:line({ { TOOL_SEPARATOR, "dim" } })
          tmp:line({})
        end
        tmp:embed(slots[i].buf)
      end
      for i = active_count + 1, #raw_calls do
        tmp:line({})
        tmp:line({ { TOOL_SEPARATOR, "dim" } })
        tmp:line({})
        tmp:line(child_header_line(calls[i].tool, "", nil, "error"))
        tmp:line({
          { CHILD_BODY_INDENT .. "[ERROR] maximum of " .. MAX_BATCH_SIZE .. " tools per batch", "error" },
        })
      end
      buf:assign(tmp)
    end

    for i = 1, active_count do
      local slot = slots[i]
      local child_id = batch_id .. CHILD_SEP .. (i - 1)
      slot.buf:on("click", function()
        if slot.click_pending then
          return
        end
        local r = slot.result
        if r and r.rendered then
          slot.rendered_expanded = not slot.rendered_expanded
          local cached = slot.rendered_cache and slot.rendered_cache[slot.rendered_expanded]
          if cached then
            r.rendered = cached
            update_slot(slot)
          else
            slot.click_pending = true
            local rendered = maki.api.render_child_output(child_id, {
              output_limit = slot.output_limit,
              expanded = slot.rendered_expanded,
              highlight = false,
            })
            if rendered then
              r.rendered = rendered
              update_slot(slot)
            end
            local expanded_cap = slot.rendered_expanded
            maki.async.run(function()
              local highlighted = maki.api.render_child_output(child_id, {
                output_limit = slot.output_limit,
                expanded = expanded_cap,
                highlight = true,
              })
              if highlighted and slot.rendered_expanded == expanded_cap then
                r.rendered = highlighted
                slot.rendered_cache = slot.rendered_cache or {}
                slot.rendered_cache[expanded_cap] = highlighted
                update_slot(slot)
              end
              slot.click_pending = false
            end)
          end
        elseif slot.view then
          slot.view:toggle()
        elseif r and r.body then
          maki.api.fire_click(child_id, 0)
        end
      end)
    end

    local funs = {}
    for i = 1, active_count do
      funs[i] = function()
        local entry = calls[i]
        local slot = slots[i]
        local child_id = batch_id .. CHILD_SEP .. (i - 1)

        if entry.tool == "batch" then
          slot.result = { output = "cannot nest batch inside batch", is_error = true }
          update_slot(slot)
          return
        end

        local reply = maki.api.call_tool(entry.tool, entry.parameters or {}, {
          id = child_id,
          parent_id = batch_id,
          on_output = function(child_buf)
            slot.result = slot.result or {}
            slot.result.body = child_buf
            update_slot(slot)
          end,
        })
        slot.result = reply

        if not slot.full_view and not reply.body then
          local max_lines = get_tool_output_limits(ctx, entry.tool)
          slot.output_limit = max_lines
          local rendered = maki.api.render_child_output(child_id, {
            output_limit = max_lines,
            highlight = false,
          })
          if rendered then
            reply.rendered = rendered
            slot.rendered_cache = { [false] = rendered }
          end
          if (not rendered or not rendered.covers_output) and reply.output and reply.output ~= "" then
            local view = ToolView.new(maki.ui.buf(), { max_lines = max_lines, keep = "head" })
            for line in reply.output:gmatch("[^\n]+") do
              view:append({ { line, reply.is_error and "error" or nil } })
            end
            view:finish()
            slot.view = view
          end
        end

        update_slot(slot)

        if not slot.full_view and not reply.body and reply.rendered then
          local child_id_cap = child_id
          local slot_cap = slot
          maki.async.run(function()
            local highlighted = maki.api.render_child_output(child_id_cap, {
              output_limit = slot_cap.output_limit,
              highlight = true,
            })
            if highlighted then
              slot_cap.result.rendered = highlighted
              slot_cap.rendered_cache = slot_cap.rendered_cache or {}
              slot_cap.rendered_cache[false] = highlighted
              update_slot(slot_cap)
            end
          end)
        end
      end
    end

    for i = 1, active_count do
      update_slot(slots[i])
    end
    compose("running")

    maki.async.join(MAX_BATCH_SIZE, funs)

    local parts = {}
    local failed = 0

    for i = 1, active_count do
      local r = slots[i].result
      parts[#parts + 1] = "## " .. calls[i].tool
      if r and r.is_error then
        failed = failed + 1
        parts[#parts + 1] = "[ERROR] " .. (r.output or "unknown error")
      else
        parts[#parts + 1] = r and r.output or ""
      end
      parts[#parts + 1] = ""
    end

    for i = active_count + 1, #raw_calls do
      failed = failed + 1
      parts[#parts + 1] = "## " .. calls[i].tool
      parts[#parts + 1] = "[ERROR] maximum of " .. MAX_BATCH_SIZE .. " tools per batch"
      parts[#parts + 1] = ""
    end

    compose(failed > 0 and "error" or "success")

    local total = #raw_calls
    local succeeded = total - failed
    if failed > 0 then
      parts[#parts + 1] = string.format("Executed %d/%d successfully. %d failed.", succeeded, total, failed)
    else
      parts[#parts + 1] = string.format("All %d tools executed successfully.", total)
    end

    return { llm_output = table.concat(parts, "\n"), body = buf }
  end,
})
