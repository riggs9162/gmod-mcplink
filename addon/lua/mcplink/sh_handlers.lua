--- Built-in operations available on both realms: status, lua and cmd.

local MAX_DEPTH = 3
local MAX_ENTRIES = 100
local MAX_PLAYERS_LISTED = 64
local CAPTURED_FUNCTIONS = {
    print = {separator = "\t", terminator = "\n"},
    Msg = {separator = "", terminator = ""},
    MsgN = {separator = "", terminator = "\n"},
    MsgC = {separator = "", terminator = ""},
}

--- Turns any Lua value into readable text, expanding tables a few levels deep.
---@realm shared
---@param value any Value to describe.
---@param depth number? Current nesting level, used when recursing into tables.
---@return string description Readable form of the value.
function mcpBridge.Describe(value, depth)
    depth = depth or 0

    if ( isstring(value) ) then
        return depth == 0 and value or string.format("%q", value)
    end

    if ( !istable(value) or IsColor(value) ) then
        return tostring(value)
    end

    if ( depth >= MAX_DEPTH ) then
        return "{...}"
    end

    local parts = {}

    for key, entry in pairs(value) do
        if ( #parts >= MAX_ENTRIES ) then
            parts[#parts + 1] = "..."

            break
        end

        parts[#parts + 1] = "[" .. mcpBridge.Describe(key, depth + 1) .. "] = " .. mcpBridge.Describe(entry, depth + 1)
    end

    return "{" .. table.concat(parts, ", ") .. "}"
end

--- Joins the printable arguments of an output function the way the console would show them.
---@param separator string Text placed between arguments.
---@param ... any Arguments passed to the output function.
---@return string text Joined text.
local function JoinArguments(separator, ...)
    local parts = {}

    for i = 1, select("#", ...) do
        local argument = select(i, ...)
        if ( !IsColor(argument) ) then
            parts[#parts + 1] = tostring(argument)
        end
    end

    return table.concat(parts, separator)
end

--- Wraps the global output functions so everything they print is also appended to a buffer.
---@param buffer string[] List that receives the printed text.
---@return fun() restore Function that puts the original output functions back.
local function CaptureOutput(buffer)
    local originals = {}

    for name, format in pairs(CAPTURED_FUNCTIONS) do
        local original = _G[name]
        originals[name] = original

        _G[name] = function(...)
            buffer[#buffer + 1] = JoinArguments(format.separator, ...) .. format.terminator

            return original(...)
        end
    end

    return function()
        for name, original in pairs(originals) do
            _G[name] = original
        end
    end
end

--- Compiles a chunk, first as an expression so a bare value such as `#player.GetAll()` is returned.
---@param code string Lua source.
---@return function|nil chunk Compiled chunk, or nil when the source does not compile.
---@return string|nil message Compile error when there is no chunk.
local function Compile(code)
    local expression = CompileString("return " .. code, "mcplink", false)
    if ( isfunction(expression) ) then
        return expression
    end

    local chunk = CompileString(code, "mcplink", false)
    if ( isfunction(chunk) ) then
        return chunk
    end

    return nil, tostring(chunk)
end

--- Builds an error report whose traceback stops at the chunk instead of continuing into the bridge.
---@param message any Error value raised by the chunk.
---@return string report Error message followed by the chunk's own stack.
local function Traceback(message)
    local trace = debug.traceback(tostring(message), 2)
    local bridgeStart = string.find(trace, "\n\t[C]: in function 'xpcall'", 1, true)

    return bridgeStart and string.sub(trace, 1, bridgeStart - 1) or trace
end

--- Gathers every value passed in, keeping trailing nils countable.
---@param ... any Values to gather.
---@return table values The values in order, with their count in `n`.
local function Collect(...)
    return {n = select("#", ...), ...}
end

--- Runs a chunk of Lua and reports what it returned and printed.
---@realm shared
---@param code string Lua source to run.
---@return table response Response with `ok`, `returns`, `output` and, on failure, `error`.
function mcpBridge.RunLua(code)
    local chunk, message = Compile(code)
    if ( !chunk ) then
        return {ok = false, error = message, output = "", returns = {}}
    end

    local output = {}
    local restore = CaptureOutput(output)
    local results = Collect(xpcall(chunk, Traceback))
    restore()

    local returns = {}

    for i = 2, results.n do
        returns[#returns + 1] = mcpBridge.Describe(results[i])
    end

    if ( !results[1] ) then
        return {ok = false, error = returns[1] or "Unknown error", output = table.concat(output), returns = {}}
    end

    return {ok = true, output = table.concat(output), returns = returns}
end

--- Runs a console command in this realm.
---@realm shared
---@param command string Console command line.
---@return table response Response with `ok` and, on failure, `error`.
function mcpBridge.RunCommand(command)
    if ( SERVER ) then
        game.ConsoleCommand(command .. "\n")

        return {ok = true}
    end

    local client = LocalPlayer()
    if ( !IsValid(client) ) then
        return {ok = false, error = "The local player is not in the game yet."}
    end

    client:ConCommand(command)

    return {ok = true}
end

--- Describes the running game for this realm.
---@realm shared
---@return table response Response with the map, gamemode, players and registered operations.
function mcpBridge.GetStatus()
    local players = {}

    for _, client in ipairs(player.GetAll()) do
        if ( #players >= MAX_PLAYERS_LISTED ) then
            break
        end

        players[#players + 1] = {
            name = client:Nick(),
            steamID = client:SteamID(),
            userID = client:UserID(),
            bBot = client:IsBot(),
        }
    end

    local operations = table.GetKeys(mcpBridge.handlers)
    table.sort(operations)

    return {
        ok = true,
        realm = SERVER and "server" or "client",
        map = game.GetMap(),
        gamemode = engine.ActiveGamemode(),
        hostname = GetHostName(),
        bDedicated = game.IsDedicated(),
        bSinglePlayer = game.SinglePlayer(),
        playerCount = player.GetCount(),
        maxPlayers = game.MaxPlayers(),
        players = players,
        uptime = math.floor(SysTime()),
        tickRate = math.Round(1 / engine.TickInterval()),
        moduleVersion = mcplink.Version,
        operations = operations,
    }
end

mcpBridge.RegisterHandler("status", function(request)
    return mcpBridge.GetStatus()
end)

mcpBridge.RegisterHandler("lua", function(request)
    if ( !isstring(request.code) ) then
        return {ok = false, error = "Missing code."}
    end

    return mcpBridge.RunLua(request.code)
end)

mcpBridge.RegisterHandler("cmd", function(request)
    if ( !isstring(request.command) ) then
        return {ok = false, error = "Missing command."}
    end

    return mcpBridge.RunCommand(request.command)
end)
