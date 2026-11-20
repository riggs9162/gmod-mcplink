--- Loads the MCP link bridge on both realms and starts it wherever the native module is installed.

AddCSLuaFile("mcplink/sh_bridge.lua")
AddCSLuaFile("mcplink/sh_handlers.lua")
AddCSLuaFile("mcplink/cl_screenshot.lua")

include("mcplink/sh_bridge.lua")
include("mcplink/sh_handlers.lua")

if ( CLIENT ) then
    include("mcplink/cl_screenshot.lua")
end

mcpBridge.Start()
