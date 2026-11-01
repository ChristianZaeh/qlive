-- qlive pandoc worker: one JSON object per line, in and out.
local json = pandoc.json

local function render(req)
  local t0 = os.clock()
  local ok, result = pcall(function()
    local doc = pandoc.read(req.text, req.from or "markdown")
    return pandoc.write(doc, req.to or "html")
  end)
  local ms = (os.clock() - t0) * 1000
  if ok then
    return { id = req.id, ok = true, out = result, ms = ms }
  end
  return { id = req.id, ok = false, err = tostring(result), ms = ms }
end

local function handle(req)
  if req.op == "ping" then
    return { id = req.id, ok = true, out = "pong", ms = 0 }
  elseif req.op == "render" then
    return render(req)
  end
  return { id = req.id, ok = false, err = "unknown op: " .. tostring(req.op) }
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
