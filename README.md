# gmod-mcplink

An [MCP](https://modelcontextprotocol.io) server for Garry's Mod that reaches the game through a small native module instead of polling files. An AI agent can run Lua, send console commands, read the console log and take screenshots in a running game, on the server realm and the client realm, with answers arriving within a frame.

## How it fits together

```
agent  <-- MCP over stdio -->  server/ (Node)  <-- loopback TCP -->  gmsv_/gmcl_mcplink DLL  <-->  addon/ (GLua)
```

| Part | Job |
| --- | --- |
| `module/` | Rust binary module. Opens a listener on `127.0.0.1`, checks a session token and hands request lines to Lua. It knows nothing about MCP or about what a request means. |
| `addon/` | GLua bridge. Polls the module every frame, runs the handler for each request and sends the answer back. All tool behaviour lives here, so it changes without recompiling anything. |
| `server/` | Node MCP server. Speaks MCP to the agent and newline-delimited JSON to the module. |

GLua has no sockets and cannot accept connections, and the game blocks HTTP requests to local addresses for clients. A pure Lua bridge is therefore left with polling files in `data/`. The module removes that limit and nothing else: it is a transport, deliberately kept thin.

## Status

Version 0.1.00, pre-production. The module, the bridge and the server have been run together end to end against the game's own LuaJIT runtime (`lua_shared.dll`, 64-bit Windows) outside the game. They have **not** yet been exercised inside a live game session, and the Linux builds are compiled by CI but untested. Expect rough edges and report what you find.

## Install

You need Node.js 20 or newer for the server.

1. **Module.** Download the DLLs for your platform from the [latest release](../../releases/latest) and put them in `garrysmod/lua/bin/` (create the folder if it does not exist). `gmsv_` files serve the server realm and `gmcl_` files the client realm; the suffix must match the game build:

   | Game build | Files |
   | --- | --- |
   | Windows, 64-bit branch | `gmsv_mcplink_win64.dll`, `gmcl_mcplink_win64.dll` |
   | Windows, default 32-bit | `gmsv_mcplink_win32.dll`, `gmcl_mcplink_win32.dll` |
   | Linux, 64-bit branch | `gmsv_mcplink_linux64.dll`, `gmcl_mcplink_linux64.dll` |
   | Linux, default 32-bit | `gmsv_mcplink_linux.dll`, `gmcl_mcplink_linux.dll` |

2. **Addon.** Copy the contents of `addon/` (or unpack `mcplink-addon.zip` from the release) into `garrysmod/addons/mcplink/`.

3. **Server.** Build it once:

   ```
   cd server
   npm install
   npm run build
   ```

4. **Register it with your agent.** For Claude Code:

   ```
   claude mcp add gmod --scope user --env GMOD_DIR="<path to>/garrysmod" -- node "<path to>/gmod-mcplink/server/dist/index.js"
   ```

   Any other MCP client works the same way: the command is `node`, the single argument is the path to `server/dist/index.js`, and `GMOD_DIR` goes in the server's environment.

5. Add `-condebug` to the game's launch options if you want `gmod_log` to work, then start the game.

On Windows, `tools/install.ps1 -GameDir "<path to>/garrysmod"` does steps 1 to 3 from source. It needs Rust.

### Configuration

| Variable | Meaning |
| --- | --- |
| `GMOD_DIR` | The `garrysmod` directory. Defaults to the standard Steam location on Windows. |
| `GMOD_SERVER_DIR` | Overrides `GMOD_DIR` for the server realm, for a dedicated server installed elsewhere. |
| `GMOD_CLIENT_DIR` | Overrides `GMOD_DIR` for the client realm. |

A listen server or single-player game needs only `GMOD_DIR`. To drive a dedicated server and a separate game client at once, point `GMOD_SERVER_DIR` and `GMOD_CLIENT_DIR` at their two directories.

## Tools

| Tool | What it does |
| --- | --- |
| `gmod_status` | Reports which realms are reachable, the map, gamemode, players and available operations. |
| `gmod_lua` | Runs a Lua chunk and returns its return values, everything it printed, and the error with a traceback if it failed. A bare expression needs no `return`. |
| `gmod_cmd` | Runs a console command in the server console or as the local player. |
| `gmod_log` | Returns the tail of `console.log`, optionally filtered by a regular expression. |
| `gmod_screenshot` | Captures the next frame the local player sees and returns it as an image. |
| `gmod_call` | Calls an operation your own addon or gamemode registered. |

`gmod_lua`, `gmod_cmd`, `gmod_log` and `gmod_call` take a `realm` of `server` (the default) or `client`.

## Adding your own operations

Any addon or gamemode can expose a purpose-built operation, which is safer and cheaper for an agent than composing raw Lua each time:

```lua
hook.Add("InitPostEntity", "myAddon.RegisterMcpOperations", function()
    if ( !mcpBridge ) then
        return
    end

    mcpBridge.RegisterHandler("inventory", function(request)
        local client = Player(request.userID)
        if ( !IsValid(client) ) then
            return {ok = false, error = "No such player."}
        end

        return {ok = true, items = myAddon.GetItemNames(client)}
    end)
end)
```

The agent calls it with `gmod_call`, passing `operation = "inventory"` and `arguments = {userID = 2}`. A handler either returns its response table, or returns nothing and calls the `reply` function it receives as its second argument later, which is how `screenshot` waits for the next frame.

## Security

This is a development tool. It runs arbitrary Lua and console commands on request, so treat the module like an open RCON port and install it only where that is acceptable.

- The listener binds to `127.0.0.1` on a random free port and is never reachable from another machine.
- Each game start generates a fresh 192-bit token. A connection must present it first, and is dropped without a reply if it is wrong or late.
- The port and token are written to `garrysmod/data/mcplink/<realm>.json`. Anything that can read that file can control the game, which already holds for any process running as your user.
- The addon does nothing on a machine that lacks the module, so players who receive the client files from your server are unaffected.
- Do not install the module on a public production server.

## Limits

- Requests are handled in the `Think` hook. A dedicated server with no players hibernates and stops thinking, so the bridge sets `sv_hibernate_think 1` when it starts.
- A paused single-player game does not think either; requests wait until it resumes.
- The token and port change on every map change. The server reconnects on the next request by itself.
- `gmod_cmd` cannot return a command's console output; read it with `gmod_log`.
- `render.Capture` is refused by the game while the main menu is open, so `gmod_screenshot` fails there.
- The menu realm is not supported.

## Protocol

For anyone writing another client. Every message is one line of UTF-8 JSON ending in `\n`.

1. Read `data/mcplink/<realm>.json` and connect to `127.0.0.1` on its `port`.
2. Send the `token` as the first line. The module answers `{"hello":"mcplink","version":"..."}` or closes the connection.
3. Send requests as `{"id": 1, "op": "lua", "code": "..."}`. Built-in operations are `status`, `lua` (`code`), `cmd` (`command`) and, on the client, `screenshot` (`quality`).
4. Each answer carries the same `id` and an `ok` flag, plus `error` when `ok` is false. Answers may arrive out of order.

## Building from source

```
cargo build --release --manifest-path module/Cargo.toml
```

The result is `module/target/release/mcplink.dll` on Windows or `libmcplink.so` on Linux. Copy it to `garrysmod/lua/bin/` twice, as `gmsv_mcplink_<suffix>.dll` and `gmcl_mcplink_<suffix>.dll`; the two are the same file. Add `--target i686-pc-windows-msvc` or `--target i686-unknown-linux-gnu` for the 32-bit game.

## License

MIT. See [LICENSE](LICENSE).
