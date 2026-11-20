--- Client operation that captures the next rendered frame as a JPEG.

local DEFAULT_QUALITY = 70
local MIN_QUALITY = 10
local MAX_QUALITY = 100

--- Captures the next rendered frame and passes it to a callback as base64 JPEG data.
---@realm client
---@param quality number JPEG quality from 10 to 100.
---@param callback fun(response: table) Receives `ok`, `width`, `height` and `data`, or `error`.
function mcpBridge.CaptureScreen(quality, callback)
    hook.Add("PostRender", "mcpBridge.CaptureScreen", function()
        hook.Remove("PostRender", "mcpBridge.CaptureScreen")

        local width, height = ScrW(), ScrH()
        local image = render.Capture({
            format = "jpeg",
            quality = quality,
            x = 0,
            y = 0,
            w = width,
            h = height,
        })

        if ( !image ) then
            callback({ok = false, error = "The game refused the capture; close the main menu and try again."})

            return
        end

        callback({
            ok = true,
            width = width,
            height = height,
            data = util.Base64Encode(image, true),
        })
    end)
end

mcpBridge.RegisterHandler("screenshot", function(request, reply)
    local quality = math.Clamp(tonumber(request.quality) or DEFAULT_QUALITY, MIN_QUALITY, MAX_QUALITY)

    mcpBridge.CaptureScreen(quality, reply)
end)
