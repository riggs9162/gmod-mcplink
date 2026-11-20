--- Routes requests from the native mcplink socket module to Lua handlers and sends their answers back.
---@class mcpBridge
---@field handlers table<string, function> Request handlers keyed by operation name.
---@field bRunning boolean Whether the listener is up and requests are being pumped.
mcpBridge = mcpBridge or {}
mcpBridge.handlers = mcpBridge.handlers or {}
mcpBridge.bRunning = mcpBridge.bRunning or false

local MODULE_NAME = "mcplink"
local REALM = SERVER and "server" or "client"
local ENDPOINT_DIRECTORY = "mcplink"
local ENDPOINT_PATH = ENDPOINT_DIRECTORY .. "/" .. REALM .. ".json"

--- Registers the function that answers one operation; it receives the request table and a reply function, and either returns the response table or returns nil and calls reply later.
---@realm shared
---@param operation string Operation name sent by the MCP server.
---@param handler fun(request: table, reply: fun(response: table)): table|nil Function that answers the operation.
function mcpBridge.RegisterHandler(operation, handler)
    mcpBridge.handlers[operation] = handler
end

--- Sends one response table to the connection that made the request.
---@realm shared
---@param clientID number Connection identifier reported by the native module.
---@param requestID number|nil Identifier the MCP server attached to the request.
---@param response table Response fields; `ok` tells the MCP server whether the request succeeded.
function mcpBridge.Reply(clientID, requestID, response)
    response.id = requestID
    mcplink.Send(clientID, util.TableToJSON(response))
end

--- Decodes one request line and runs its handler, turning a handler error into a failed response.
---@realm shared
---@param clientID number Connection identifier reported by the native module.
---@param line string JSON request line.
function mcpBridge.Dispatch(clientID, line)
    local request = util.JSONToTable(line, true, true)
    if ( !istable(request) ) then
        return
    end

    local requestID = request.id

    local function reply(response)
        mcpBridge.Reply(clientID, requestID, response)
    end

    local handler = mcpBridge.handlers[request.op]
    if ( !handler ) then
        reply({ok = false, error = "Unknown operation: " .. tostring(request.op)})

        return
    end

    local bOk, response = xpcall(handler, debug.traceback, request, reply)
    if ( !bOk ) then
        reply({ok = false, error = response})

        return
    end

    if ( istable(response) ) then
        reply(response)
    end
end

--- Handles every request that arrived since the previous frame.
---@realm shared
function mcpBridge.Pump()
    local messages = mcplink.Poll()

    for i = 1, #messages do
        mcpBridge.Dispatch(messages[i].client, messages[i].data)
    end
end

--- Starts the loopback listener and publishes its port and token for MCP servers to read; does nothing when the native module is not installed.
---@realm shared
---@return boolean bStarted Whether the bridge is running.
function mcpBridge.Start()
    if ( !util.IsBinaryModuleInstalled(MODULE_NAME) ) then
        return false
    end

    require(MODULE_NAME)

    if ( !istable(mcplink) ) then
        ErrorNoHalt("[mcplink] The native module loaded but could not reach the Lua runtime.\n")

        return false
    end

    local port, token = mcplink.Listen()
    if ( !port ) then
        ErrorNoHalt("[mcplink] Could not start the listener: " .. tostring(token) .. "\n")

        return false
    end

    file.CreateDir(ENDPOINT_DIRECTORY)
    file.Write(ENDPOINT_PATH, util.TableToJSON({
        port = port,
        token = token,
        realm = REALM,
        version = mcplink.Version,
    }))

    hook.Add("Think", "mcpBridge.Think", function()
        mcpBridge.Pump()
    end)

    hook.Add("ShutDown", "mcpBridge.ShutDown", function()
        mcpBridge.Stop()
    end)

    if ( SERVER ) then
        RunConsoleCommand("sv_hibernate_think", "1")
    end

    mcpBridge.bRunning = true

    return true
end

--- Stops the listener and removes the published endpoint.
---@realm shared
function mcpBridge.Stop()
    hook.Remove("Think", "mcpBridge.Think")
    hook.Remove("ShutDown", "mcpBridge.ShutDown")

    if ( istable(mcplink) ) then
        mcplink.Close()
    end

    file.Delete(ENDPOINT_PATH)
    mcpBridge.bRunning = false
end
