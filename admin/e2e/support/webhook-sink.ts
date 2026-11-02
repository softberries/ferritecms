// A local webhook receiver for `e2e/webhooks.spec.ts` (M6-6): a plain-HTTP server on
// loopback (the E2E site runs with `[webhooks] local_targets`), recording every request
// and answering the status the test sets. `verify` checks a Standard Webhooks signature
// with Node's own HMAC, independent of the server's code.

import { createHmac } from "node:crypto";
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";

export interface Hit {
  headers: Record<string, string>;
  body: string;
  json: Record<string, unknown> & { type?: string; data?: Record<string, unknown> };
}

export interface WebhookSink {
  url: string;
  hits: Hit[];
  /** The status every next request gets (default 200). */
  answer: number;
  stop(): Promise<void>;
}

function read(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

export async function startWebhookSink(): Promise<WebhookSink> {
  const sink: WebhookSink = {
    url: "",
    hits: [],
    answer: 200,
    stop: async () => {},
  };
  const server = createServer((req, res) => {
    void read(req).then((body) => {
      const headers: Record<string, string> = {};
      for (const [k, v] of Object.entries(req.headers)) {
        if (typeof v === "string") headers[k] = v;
      }
      let json: Hit["json"] = {};
      try {
        json = JSON.parse(body) as Hit["json"];
      } catch {
        // Recorded as it came.
      }
      sink.hits.push({ headers, body, json });
      res.writeHead(sink.answer, { "Content-Type": "text/plain" });
      res.end(sink.answer >= 400 ? "receiver says no" : "ok");
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  sink.url = `http://127.0.0.1:${port}/hook`;
  sink.stop = () => new Promise((resolve) => server.close(() => resolve()));
  return sink;
}

/** The key bytes of a `whsec_` secret. */
function keyOf(secret: string): Buffer {
  return Buffer.from(secret.replace(/^whsec_/, ""), "base64");
}

/** The `v1` signature of `hit` under `secret`. */
export function signatureOf(hit: Hit, secret: string): string {
  const id = hit.headers["webhook-id"] ?? "";
  const ts = hit.headers["webhook-timestamp"] ?? "";
  const mac = createHmac("sha256", keyOf(secret))
    .update(`${id}.${ts}.${hit.body}`)
    .digest("base64");
  return `v1,${mac}`;
}

/** Whether one of `hit`'s signatures is `secret`'s. */
export function verify(hit: Hit, secret: string): boolean {
  const want = signatureOf(hit, secret);
  return (hit.headers["webhook-signature"] ?? "").split(" ").includes(want);
}
