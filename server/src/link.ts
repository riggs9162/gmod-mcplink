import { readFile } from "node:fs/promises";
import { createConnection, type Socket } from "node:net";
import { join } from "node:path";

/** Game realm a request is sent to. */
export type Realm = "server" | "client";

/** Response table sent back by the in-game bridge. */
export interface GameResponse {
  id?: number;
  ok?: boolean;
  error?: string;
  [field: string]: unknown;
}

/** Raised when the game cannot be reached, as opposed to the game answering with a failure. */
export class GameUnavailableError extends Error {}

interface Endpoint {
  port: number;
  token: string;
}

interface PendingRequest {
  resolve: (response: GameResponse) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

const HANDSHAKE_TIMEOUT_MS = 3000;
const LINE_FEED = 0x0a;

/** One lazily opened, automatically reopened connection to the socket module of a single realm. */
export class GameLink {
  private socket: Socket | null = null;
  private connecting: Promise<Socket> | null = null;
  private readonly pending = new Map<number, PendingRequest>();
  private nextId = 1;

  /**
   * @param realm Realm this link talks to.
   * @param gameDir The `garrysmod` directory whose `data/mcplink` folder holds the realm's endpoint file.
   */
  constructor(
    readonly realm: Realm,
    readonly gameDir: string,
  ) {}

  /**
   * Sends one operation to the game and waits for its answer.
   * @param op Operation name registered with the in-game bridge.
   * @param payload Extra request fields.
   * @param timeoutMs How long to wait for the answer.
   * @throws GameUnavailableError when the game is not running, not reachable or does not answer in time.
   */
  async request(op: string, payload: Record<string, unknown>, timeoutMs: number): Promise<GameResponse> {
    const socket = await this.connect();
    const id = this.nextId++;

    return new Promise<GameResponse>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new GameUnavailableError(`The ${this.realm} realm did not answer within ${timeoutMs} ms.`));
      }, timeoutMs);

      this.pending.set(id, { resolve, reject, timer });
      socket.write(`${JSON.stringify({ ...payload, id, op })}\n`);
    });
  }

  /** Closes the connection, failing any request still waiting for an answer. */
  close(): void {
    this.socket?.destroy();
  }

  /** Returns the open socket, opening and authenticating a new one when there is none. */
  private connect(): Promise<Socket> {
    if (this.socket) {
      return Promise.resolve(this.socket);
    }

    this.connecting ??= this.open().finally(() => {
      this.connecting = null;
    });

    return this.connecting;
  }

  /** Reads the port and token the game published for this realm. */
  private async readEndpoint(): Promise<Endpoint> {
    const path = join(this.gameDir, "data", "mcplink", `${this.realm}.json`);

    try {
      const endpoint = JSON.parse(await readFile(path, "utf8")) as Partial<Endpoint>;
      const port = Number(endpoint.port);
      if (!Number.isInteger(port) || typeof endpoint.token !== "string") {
        throw new Error("malformed endpoint file");
      }

      return { port, token: endpoint.token };
    } catch {
      throw new GameUnavailableError(
        `The ${this.realm} realm is not running, or the mcplink addon and module are not installed (no usable ${path}).`,
      );
    }
  }

  /** Connects to the published endpoint, presents the token and waits for the module's greeting. */
  private async open(): Promise<Socket> {
    const endpoint = await this.readEndpoint();

    return new Promise<Socket>((resolve, reject) => {
      const socket = createConnection({ host: "127.0.0.1", port: endpoint.port });
      const chunks: Buffer[] = [];
      let greeted = false;

      const unavailable = new GameUnavailableError(
        `The ${this.realm} realm is not accepting connections; it has probably shut down.`,
      );
      const handshake = setTimeout(() => socket.destroy(), HANDSHAKE_TIMEOUT_MS);

      const handleLine = (line: string): void => {
        if (!greeted) {
          greeted = true;
          clearTimeout(handshake);
          this.socket = socket;
          resolve(socket);

          return;
        }

        this.settle(line);
      };

      socket.setNoDelay(true);
      socket.once("connect", () => socket.write(`${endpoint.token}\n`));
      socket.on("error", () => {});
      socket.on("data", (chunk: Buffer) => {
        let start = 0;
        let end = chunk.indexOf(LINE_FEED);

        while (end !== -1) {
          chunks.push(chunk.subarray(start, end));
          handleLine(Buffer.concat(chunks).toString("utf8"));
          chunks.length = 0;
          start = end + 1;
          end = chunk.indexOf(LINE_FEED, start);
        }

        if (start < chunk.length) {
          chunks.push(chunk.subarray(start));
        }
      });
      socket.once("close", () => {
        clearTimeout(handshake);

        if (this.socket === socket) {
          this.socket = null;
        }

        this.failPending(unavailable);
        reject(unavailable);
      });
    });
  }

  /** Resolves the request a response line belongs to. */
  private settle(line: string): void {
    let response: GameResponse;

    try {
      response = JSON.parse(line) as GameResponse;
    } catch {
      return;
    }

    const id = Number(response.id);
    const request = this.pending.get(id);
    if (!request) {
      return;
    }

    this.pending.delete(id);
    clearTimeout(request.timer);
    request.resolve(response);
  }

  /** Rejects every request still waiting for an answer. */
  private failPending(error: Error): void {
    for (const request of this.pending.values()) {
      clearTimeout(request.timer);
      request.reject(error);
    }

    this.pending.clear();
  }
}
