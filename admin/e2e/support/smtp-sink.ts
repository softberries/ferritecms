// A minimal SMTP server for the E2E sites' `[mail] smtp_url` (M2b P23): EHLO/HELO, MAIL,
// RCPT, DATA, RSET, NOOP, QUIT over `node:net`; no AUTH, no TLS (plain `smtp://` to a
// loopback relay is what `[mail]` accepts, 01 §4). It accepts every message.
//
// The sink runs in the global-setup process and the tests in worker processes, so each
// message is also written as JSON to `dir`; `mailbox(dir)` reads them back in a worker.

import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { createServer, type AddressInfo, type Socket } from "node:net";
import { join } from "node:path";

export interface Mail {
  from: string;
  to: string[];
  headers: Record<string, string>;
  /** The body after undoing the transfer encoding (quoted-printable or base64). */
  text: string;
  /** Milliseconds since the epoch. */
  receivedAt: number;
}

export interface SmtpSink {
  port: number;
  messages: () => Mail[];
  waitFor(to: string, ms?: number): Promise<Mail>;
  stop(): Promise<void>;
}

export async function startSmtpSink(opts: { dir?: string } = {}): Promise<SmtpSink> {
  const received: Mail[] = [];
  if (opts.dir) mkdirSync(opts.dir, { recursive: true });
  const sockets = new Set<Socket>();
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    session(socket, (mail) => {
      received.push(mail);
      if (opts.dir) {
        const name = `${String(received.length).padStart(5, "0")}.json`;
        writeFileSync(join(opts.dir, name), JSON.stringify(mail));
      }
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve());
  });
  const { port } = server.address() as AddressInfo;
  return {
    port,
    messages: () => [...received],
    waitFor: (to, ms) => poll(() => received, to, ms ?? 15_000, 0),
    stop: () =>
      new Promise<void>((resolve) => {
        for (const s of sockets) s.destroy();
        server.close(() => resolve());
      }),
  };
}

/** The messages a sink started with `{ dir }` has received, read from another process. */
export function mailbox(dir: string) {
  const messages = (): Mail[] => {
    let names: string[];
    try {
      names = readdirSync(dir).filter((n) => n.endsWith(".json"));
    } catch {
      return [];
    }
    return names.sort().map((n) => JSON.parse(readFileSync(join(dir, n), "utf8")) as Mail);
  };
  return {
    messages,
    /** The first mail to `to` received at or after `after` (ms since the epoch). */
    waitFor: (to: string, opts: { ms?: number; after?: number } = {}) =>
      poll(messages, to, opts.ms ?? 15_000, opts.after ?? 0),
  };
}

async function poll(list: () => Mail[], to: string, ms: number, after: number): Promise<Mail> {
  const want = to.toLowerCase();
  const deadline = Date.now() + ms;
  for (;;) {
    const hit = list().find(
      (m) => m.receivedAt >= after && m.to.some((r) => r.toLowerCase() === want),
    );
    if (hit) return hit;
    if (Date.now() > deadline) throw new Error(`no mail to ${to} within ${ms} ms`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

function session(socket: Socket, deliver: (mail: Mail) => void) {
  let buf = "";
  let from = "";
  let to: string[] = [];
  let data: string[] | null = null;
  const reply = (line: string) => socket.write(`${line}\r\n`);
  const reset = () => {
    from = "";
    to = [];
    data = null;
  };
  socket.setEncoding("utf8");
  socket.on("error", () => socket.destroy());
  reply("220 ferrite-e2e-sink ESMTP");
  socket.on("data", (chunk: string) => {
    buf += chunk;
    let i: number;
    while ((i = buf.indexOf("\r\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 2);
      if (data) {
        if (line === ".") {
          deliver(parse(from, to, data.join("\r\n")));
          reset();
          reply("250 queued");
        } else {
          // Dot-stuffing (RFC 5321 §4.5.2).
          data.push(line.startsWith(".") ? line.slice(1) : line);
        }
        continue;
      }
      const upper = line.toUpperCase();
      if (upper.startsWith("EHLO") || upper.startsWith("HELO")) {
        reset();
        reply("250 ferrite-e2e-sink");
      } else if (upper.startsWith("MAIL FROM:")) {
        from = address(line.slice(10));
        reply("250 ok");
      } else if (upper.startsWith("RCPT TO:")) {
        to.push(address(line.slice(8)));
        reply("250 ok");
      } else if (upper === "DATA") {
        if (to.length === 0) {
          reply("503 no recipients");
        } else {
          data = [];
          reply("354 end with <CRLF>.<CRLF>");
        }
      } else if (upper === "RSET") {
        reset();
        reply("250 ok");
      } else if (upper === "NOOP") {
        reply("250 ok");
      } else if (upper === "QUIT") {
        reply("221 bye");
        socket.end();
      } else {
        reply("502 not implemented");
      }
    }
  });
}

function address(arg: string): string {
  const m = /<([^>]*)>/.exec(arg);
  return (m?.[1] ?? arg.trim().split(/\s+/)[0] ?? "").trim();
}

function parse(from: string, to: string[], raw: string): Mail {
  const split = raw.indexOf("\r\n\r\n");
  const head = split >= 0 ? raw.slice(0, split) : raw;
  const body = split >= 0 ? raw.slice(split + 4) : "";
  const headers: Record<string, string> = {};
  // Unfold continuation lines, then split `Name: value`.
  for (const line of head.replace(/\r\n[ \t]+/g, " ").split("\r\n")) {
    const c = line.indexOf(":");
    if (c > 0) headers[line.slice(0, c).trim().toLowerCase()] = line.slice(c + 1).trim();
  }
  const cte = (headers["content-transfer-encoding"] ?? "").toLowerCase();
  let text = body;
  if (cte === "quoted-printable") {
    text = Buffer.from(
      body
        .replace(/=\r\n/g, "")
        .replace(/=([0-9A-Fa-f]{2})/g, (_, h: string) => String.fromCharCode(parseInt(h, 16))),
      "latin1",
    ).toString("utf8");
  } else if (cte === "base64") {
    text = Buffer.from(body.replace(/\s+/g, ""), "base64").toString("utf8");
  }
  return { from, to, headers, text, receivedAt: Date.now() };
}
