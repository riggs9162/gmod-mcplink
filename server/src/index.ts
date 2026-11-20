#!/usr/bin/env node
import { open, stat } from "node:fs/promises";
import { join } from "node:path";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

import { GameLink, GameUnavailableError, type GameResponse, type Realm } from "./link.js";

const DEFAULT_GAME_DIR = "C:/Program Files (x86)/Steam/steamapps/common/GarrysMod/garrysmod";
const STATUS_TIMEOUT_MS = 2000;
const COMMAND_TIMEOUT_MS = 5000;
const SCREENSHOT_TIMEOUT_MS = 10000;
const LOG_TAIL_BYTES = 1024 * 1024;
const MAX_TEXT_LENGTH = 60000;

const gameDir = process.env.GMOD_DIR ?? DEFAULT_GAME_DIR;
const gameDirs: Record<Realm, string> = {
  server: process.env.GMOD_SERVER_DIR ?? gameDir,
  client: process.env.GMOD_CLIENT_DIR ?? gameDir,
};
const links: Record<Realm, GameLink> = {
  server: new GameLink("server", gameDirs.server),
  client: new GameLink("client", gameDirs.client),
};

const realmSchema = z
  .enum(["server", "client"])
  .default("server")
  .describe("server = server realm, client = client realm of the local player");

/** Wraps plain text as a tool result, shortening anything too long for a model to read comfortably. */
function text(body: string, isError = false): CallToolResult {
  const shown =
    body.length > MAX_TEXT_LENGTH
      ? `${body.slice(0, MAX_TEXT_LENGTH)}\n... (${body.length - MAX_TEXT_LENGTH} more characters cut)`
      : body;

  return { content: [{ type: "text", text: shown }], isError };
}

/** Runs a tool body, reporting an unreachable game as a tool error instead of a protocol failure. */
async function guarded(run: () => Promise<CallToolResult>): Promise<CallToolResult> {
  try {
    return await run();
  } catch (error) {
    if (error instanceof GameUnavailableError) {
      return text(error.message, true);
    }

    throw error;
  }
}

/** Normalises a list the game encoded as either a JSON array or an object with numeric keys. */
function toList(value: unknown): unknown[] {
  if (Array.isArray(value)) {
    return value;
  }

  return value && typeof value === "object" ? Object.values(value) : [];
}

/** Formats the answer of a Lua run: return values, printed output and any error. */
function describeLuaRun(response: GameResponse): string {
  const sections: string[] = [];
  const returns = toList(response.returns).map(String);
  const output = typeof response.output === "string" ? response.output.trimEnd() : "";

  if (!response.ok) {
    sections.push(`Lua error:\n${response.error ?? "unknown error"}`);
  }

  if (returns.length > 0) {
    sections.push(`Returned:\n${returns.join("\n")}`);
  }

  if (output.length > 0) {
    sections.push(`Printed:\n${output}`);
  }

  return sections.length > 0 ? sections.join("\n\n") : "Ran with no return value and no output.";
}

/** Reads the last lines of a realm's console.log, optionally keeping only those matching a pattern. */
async function tailLog(realm: Realm, lines: number, filter: string | undefined): Promise<string> {
  const path = join(gameDirs[realm], "console.log");
  const pattern = filter === undefined ? null : new RegExp(filter, "i");
  const file = await open(path, "r");

  try {
    const { size } = await file.stat();
    const length = Math.min(size, LOG_TAIL_BYTES);
    const buffer = Buffer.alloc(length);
    await file.read(buffer, 0, length, size - length);

    const kept = buffer
      .toString("utf8")
      .split(/\r?\n/)
      .slice(length < size ? 1 : 0)
      .filter((line) => line.length > 0 && (pattern === null || pattern.test(line)));

    return kept.slice(-lines).join("\n");
  } finally {
    await file.close();
  }
}

/** Summarises whether one realm is reachable and what it is running. */
async function describeRealm(realm: Realm): Promise<string> {
  try {
    const status = await links[realm].request("status", {}, STATUS_TIMEOUT_MS);

    return `${realm}: running\n${JSON.stringify({ ...status, id: undefined, ok: undefined }, null, 2)}`;
  } catch (error) {
    if (error instanceof GameUnavailableError) {
      return `${realm}: not reachable. ${error.message}`;
    }

    throw error;
  }
}

/** Summarises whether a realm's console.log exists and how recently it was written. */
async function describeLog(realm: Realm): Promise<string> {
  const path = join(gameDirs[realm], "console.log");

  try {
    const info = await stat(path);

    return `${realm} console.log: ${info.size} bytes, last written ${info.mtime.toISOString()}`;
  } catch {
    return `${realm} console.log: missing (${path}). Add -condebug to the launch options to create it.`;
  }
}

const server = new McpServer({ name: "gmod-mcplink", version: "0.1.1" });

server.registerTool(
  "gmod_status",
  {
    title: "Garry's Mod status",
    description:
      "Report which realms are reachable, the map, gamemode, players and registered operations, and whether console.log exists. Call this first when another tool fails.",
    inputSchema: {},
  },
  async () => {
    const realms = await Promise.all([describeRealm("server"), describeRealm("client")]);
    const distinctLogs: Realm[] = gameDirs.server === gameDirs.client ? ["server"] : ["server", "client"];
    const logs = await Promise.all(distinctLogs.map(describeLog));

    return text([...realms, ...logs].join("\n\n"));
  },
);

server.registerTool(
  "gmod_lua",
  {
    title: "Run Lua in Garry's Mod",
    description:
      "Compile and run a Lua chunk inside the running game. A bare expression such as `#player.GetAll()` is returned without needing `return`. The result holds the return values, anything printed with print, Msg, MsgN or MsgC, and the error with a traceback if the chunk failed.",
    inputSchema: {
      code: z.string().describe("Lua source to run"),
      realm: realmSchema,
      timeout_ms: z.number().int().min(500).max(120000).default(COMMAND_TIMEOUT_MS),
    },
  },
  ({ code, realm, timeout_ms }) =>
    guarded(async () => text(describeLuaRun(await links[realm].request("lua", { code }, timeout_ms)))),
);

server.registerTool(
  "gmod_cmd",
  {
    title: "Run a console command",
    description:
      "Run a console command in the server console or as the local player. The command's own console output is not returned; read it with gmod_log.",
    inputSchema: {
      command: z.string().describe("Console command line, for example: changelevel gm_construct"),
      realm: realmSchema,
    },
  },
  ({ command, realm }) =>
    guarded(async () => {
      const response = await links[realm].request("cmd", { command }, COMMAND_TIMEOUT_MS);

      return response.ok ? text("Command sent.") : text(response.error ?? "The command was rejected.", true);
    }),
);

server.registerTool(
  "gmod_call",
  {
    title: "Call a custom operation",
    description:
      "Call an operation that an addon or gamemode registered in the game with mcpBridge.RegisterHandler. gmod_status lists the operations each realm offers.",
    inputSchema: {
      operation: z.string().describe("Operation name"),
      arguments: z.record(z.string(), z.unknown()).default({}).describe("Fields passed to the handler"),
      realm: realmSchema,
      timeout_ms: z.number().int().min(500).max(120000).default(COMMAND_TIMEOUT_MS),
    },
  },
  ({ operation, arguments: fields, realm, timeout_ms }) =>
    guarded(async () => {
      const response = await links[realm].request(operation, fields, timeout_ms);

      return text(JSON.stringify({ ...response, id: undefined }, null, 2), response.ok === false);
    }),
);

server.registerTool(
  "gmod_log",
  {
    title: "Read the console log",
    description:
      "Return the last lines of garrysmod/console.log, optionally keeping only lines that match a case-insensitive regular expression. The game writes this file only when launched with -condebug.",
    inputSchema: {
      lines: z.number().int().min(1).max(1000).default(80),
      filter: z.string().optional().describe("Case-insensitive regular expression"),
      realm: realmSchema.describe("Whose console.log to read when the server and client use different directories"),
    },
  },
  async ({ lines, filter, realm }) => {
    try {
      const tail = await tailLog(realm, lines, filter);

      return text(tail.length > 0 ? tail : "No matching lines.");
    } catch (error) {
      return text(`Could not read console.log: ${error instanceof Error ? error.message : String(error)}`, true);
    }
  },
);

server.registerTool(
  "gmod_screenshot",
  {
    title: "Screenshot the game",
    description:
      "Capture the next frame the local player sees, including the HUD and open menus, and return it as a JPEG image.",
    inputSchema: {
      quality: z.number().int().min(10).max(100).default(70).describe("JPEG quality"),
    },
  },
  ({ quality }) =>
    guarded(async () => {
      const response = await links.client.request("screenshot", { quality }, SCREENSHOT_TIMEOUT_MS);
      if (!response.ok || typeof response.data !== "string") {
        return text(response.error ?? "The game returned no image.", true);
      }

      return {
        content: [
          { type: "image", data: response.data, mimeType: "image/jpeg" },
          { type: "text", text: `${Number(response.width)}x${Number(response.height)}` },
        ],
      };
    }),
);

await server.connect(new StdioServerTransport());
