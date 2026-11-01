-- qlive pandoc worker: one JSON object per line, in and out.
--
-- Plain ops:   ping, render          (pandoc read + write, no filters)
-- Quarto ops:  quarto_init, quarto_render
--
-- quarto_init loads Quarto's own pipeline into this process once: its datadir
-- init.lua, its qmd reader, and (per render) its main.lua filter chain. It needs
-- the environment from a captured `quarto render` (QUARTO_SHARE_PATH,
-- QUARTO_FILTER_PARAMS, ...), which the Rust side passes when spawning.
-- After quarto_init the process is in Quarto mode for good: init.lua patches
-- require, io.input and friends globally.
local json = pandoc.json

local quarto = nil

local function plain_render(req)
  local t0 = os.clock()
  local doc = pandoc.read(req.text, req.from or "markdown")
  local out = pandoc.write(doc, req.to or "html")
  return { id = req.id, ok = true, out = out, ms = (os.clock() - t0) * 1000 }
end

local function quarto_init(req)
  local share = os.getenv("QUARTO_SHARE_PATH")
  if not share then
    error("QUARTO_SHARE_PATH is not set; spawn the worker with a captured Quarto env")
  end
  local datadir = share .. "/pandoc/datadir"
  local filters = share .. "/filters"
  package.path = datadir .. "/?.lua;" .. filters .. "/?.lua;" .. package.path

  -- `pandoc lua` sets none of these; Quarto's filters read all of them.
  FORMAT = req.format or "html"
  -- The pandoc CLI reads with --columns (default 72); the Lua constructor defaults
  -- to 80. Pipe-table widths are dash counts over this, so 80 shrinks every column.
  PANDOC_READER_OPTIONS = pandoc.ReaderOptions({ columns = 72 })
  PANDOC_WRITER_OPTIONS = pandoc.WriterOptions({
    section_divs = true,
    html_math_method = "mathjax",
  })

  -- init.lua builds paths from PANDOC_STATE.user_data_dir, which only --data-dir
  -- sets. The field is read-only, so shadow the global and fall through for the rest.
  local real_state = PANDOC_STATE
  PANDOC_STATE = setmetatable({ user_data_dir = datadir }, { __index = real_state })

  dofile(datadir .. "/init.lua")       -- param(), quarto.*, normally loaded via --data-dir
  dofile(filters .. "/qmd-reader.lua") -- defines Reader(); pandoc.read rejects custom readers

  quarto = { main = filters .. "/main.lua", citeproc = req.citeproc == true }
  return { id = req.id, ok = true }
end

local function quarto_render(req)
  if not quarto then
    error("quarto_init has not run")
  end
  local t0 = os.clock()
  local doc = Reader(req.text, PANDOC_READER_OPTIONS)
  if req.meta and req.meta ~= "" then
    -- Quarto passes these with --metadata-file; the title lives here, not in the input.
    local meta = pandoc.read("---\n" .. req.meta .. "\n---\n", "markdown").meta
    for k, v in pairs(meta) do
      doc.meta[k] = v
    end
  end
  -- Run in _G: under real pandoc, init.lua and the filter share one global
  -- environment, and _utils.lua (loaded by init.lua) calls globals main.lua defines.
  -- pandoc sets this to the running filter; init.lua resolves relative paths from it.
  PANDOC_SCRIPT_FILE = quarto.main
  doc = pandoc.utils.run_lua_filter(doc, quarto.main, _G)
  -- Quarto's filter list ends with citeproc, after main.lua.
  if quarto.citeproc then
    doc = pandoc.utils.citeproc(doc)
  end
  local out = pandoc.write(doc, FORMAT, PANDOC_WRITER_OPTIONS)
  return { id = req.id, ok = true, out = out, ms = (os.clock() - t0) * 1000 }
end

local handlers = {
  ping = function(req) return { id = req.id, ok = true, out = "pong", ms = 0 } end,
  render = plain_render,
  quarto_init = quarto_init,
  quarto_render = quarto_render,
}

local function handle(req)
  local handler = handlers[req.op]
  if not handler then
    return { id = req.id, ok = false, err = "unknown op: " .. tostring(req.op) }
  end
  local ok, result = pcall(handler, req)
  if ok then
    return result
  end
  return { id = req.id, ok = false, err = tostring(result) }
end

io.stdout:setvbuf("line")
while true do
  local line = io.read("l")
  if line == nil then break end
  line = line:gsub("\r$", "")
  if line ~= "" then
    local parsed, req = pcall(json.decode, line)
    local response
    if parsed and type(req) == "table" then
      response = handle(req)
    else
      response = { ok = false, err = "bad request json" }
    end
    io.write(json.encode(response), "\n")
    io.stdout:flush()
  end
end
