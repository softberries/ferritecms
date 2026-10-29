import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { setCsrfToken } from "@/api/csrf";
import { renderWithRouter } from "@/test/render";
import { StartCard } from "./start-card";

/** Every XHR the card opens, in order. */
const sent: FakeXhr[] = [];

class FakeXhr {
  url = "";
  headers: Record<string, string> = {};
  body: FormData | null = null;
  status = 0;
  responseText = "";
  withCredentials = false;
  upload: { onprogress: ((e: ProgressEvent) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  responseHeaders: Record<string, string> = {};
  open(_method: string, url: string) {
    this.url = url;
  }
  setRequestHeader(k: string, v: string) {
    this.headers[k] = v;
  }
  getResponseHeader(k: string) {
    return this.responseHeaders[k] ?? null;
  }
  send(body: FormData) {
    this.body = body;
    sent.push(this);
  }
  abort() {
    this.onabort?.();
  }
  respond(status: number, body: unknown, headers: Record<string, string> = {}) {
    this.status = status;
    this.responseText = JSON.stringify(body);
    this.responseHeaders = headers;
    this.onload?.();
  }
}

beforeEach(() => {
  sent.length = 0;
  vi.stubGlobal("XMLHttpRequest", FakeXhr);
  setCsrfToken("tok");
});
afterEach(() => {
  vi.unstubAllGlobals();
  setCsrfToken(null);
});

const WXR = new File(["<rss/>"], "site-a.xml", { type: "text/xml" });
const ZIP = new File(["PK"], "uploads.zip", { type: "application/zip" });

function input(slot: string): HTMLInputElement {
  const el = document.querySelector<HTMLInputElement>(`[data-slot=${slot}] input[type=file]`);
  if (!el) throw new Error(`no file input in ${slot}`);
  return el;
}

async function choose() {
  const user = userEvent.setup();
  await user.upload(input("import-wxr"), WXR);
  await user.upload(input("import-uploads"), ZIP);
  return user;
}

test("start sends wxr, zip and options with an Idempotency-Key and retries with the same key", async () => {
  const onStarted = vi.fn();
  renderWithRouter(<StartCard onStarted={onStarted} />);
  const start = await screen.findByRole("button", { name: "Start import" });
  expect(start).toBeDisabled();
  const user = await choose();
  expect(screen.getByText("site-a.xml")).toBeInTheDocument();
  expect(screen.getByText("uploads.zip")).toBeInTheDocument();

  await user.click(screen.getByRole("switch", { name: "Fetch missing media" }));
  await user.click(screen.getByRole("button", { name: "Advanced" }));
  const tagBase = screen.getByRole("textbox", { name: "Tag base" });
  await user.clear(tagBase);
  await user.type(tagBase, "topics");
  await user.click(screen.getByRole("button", { name: "Start import" }));

  expect(sent).toHaveLength(1);
  const first = sent[0]!;
  expect(first.url).toBe("/api/v1/import/wordpress");
  const key = first.headers["Idempotency-Key"];
  expect(key).toMatch(/^[0-9a-f-]{36}$/);
  const form = first.body!;
  expect([...form.keys()]).toEqual(["options", "uploads", "wxr"]);
  expect(JSON.parse(form.get("options") as string)).toEqual({
    dry_run: false,
    keep_permalinks: true,
    site_options: true,
    fetch_media: true,
    category_base: "category",
    tag_base: "topics",
  });
  expect((form.get("wxr") as File).name).toBe("site-a.xml");
  expect((form.get("uploads") as File).name).toBe("uploads.zip");
  // Progress while the files go up; the form cannot change meanwhile.
  first.upload.onprogress?.({ lengthComputable: true, loaded: 1, total: 4 } as ProgressEvent);
  expect(await screen.findByText("Uploading 25%")).toBeInTheDocument();
  expect(screen.getByRole("progressbar", { name: "Upload progress" })).toHaveAttribute(
    "aria-valuenow",
    "25",
  );
  expect(screen.getByRole("switch", { name: "Dry run" })).toBeDisabled();

  // No answer: Retry sends the same files under the same key.
  first.onerror?.();
  expect(await screen.findByRole("alert")).toHaveTextContent(/no answer from the server/);
  await user.click(screen.getByRole("button", { name: "Retry" }));
  expect(sent).toHaveLength(2);
  expect(sent[1]!.headers["Idempotency-Key"]).toBe(key);

  // An unreadable `2xx`: still the same key.
  sent[1]!.status = 202;
  sent[1]!.responseText = "<html>";
  sent[1]!.onload?.();
  await user.click(await screen.findByRole("button", { name: "Retry" }));
  expect(sent[2]!.headers["Idempotency-Key"]).toBe(key);
  sent[2]!.onerror?.();

  // Changing an option makes it a new request.
  await user.click(await screen.findByRole("switch", { name: "Dry run" }));
  await user.click(screen.getByRole("button", { name: "Start dry run" }));
  const fresh = sent[3]!.headers["Idempotency-Key"];
  expect(fresh).not.toBe(key);
  expect(JSON.parse(sent[3]!.body!.get("options") as string)).toMatchObject({ dry_run: true });
  sent[3]!.respond(202, { job_id: "j-1", status: "queued", attempts: 0 });
  await vi.waitFor(() => expect(onStarted).toHaveBeenCalledTimes(1));
  expect(onStarted.mock.calls[0]?.[0]).toMatchObject({ job_id: "j-1" });
});

test("busy and refused starts say why and take a new key", async () => {
  const onBusy = vi.fn();
  renderWithRouter(<StartCard onStarted={vi.fn()} onBusy={onBusy} />);
  await screen.findByRole("button", { name: "Start import" });
  const user = await choose();
  await user.click(screen.getByRole("button", { name: "Start import" }));
  const problem = { "Content-Type": "application/problem+json" };
  sent[0]!.respond(
    409,
    { type: "/api/v1/problems/conflict", title: "Conflict", status: 409, detail: "busy" },
    problem,
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Another import is queued or running.",
  );
  expect(onBusy).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole("button", { name: "Start import" }));
  expect(sent[1]!.headers["Idempotency-Key"]).not.toBe(sent[0]!.headers["Idempotency-Key"]);

  // A base the server refuses is shown at its field, under Advanced.
  sent[1]!.respond(
    422,
    {
      type: "/api/v1/problems/validation",
      title: "Validation",
      status: 422,
      detail: "bad",
      errors: [{ field: "category_base", detail: "not a path" }],
    },
    problem,
  );
  expect(await screen.findByText("not a path")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Category base" })).toHaveAttribute(
    "aria-invalid",
    "true",
  );
});

test("bad bases and wrong files are refused before sending", async () => {
  renderWithRouter(<StartCard onStarted={vi.fn()} />);
  await screen.findByRole("button", { name: "Start import" });
  const user = userEvent.setup({ applyAccept: false });
  await user.upload(input("import-wxr"), new File(["x"], "notes.txt", { type: "text/plain" }));
  expect(
    within(document.querySelector<HTMLElement>("[data-slot=import-wxr]")!).getByRole("alert"),
  ).toHaveTextContent("notes.txt is not an .xml export.");
  await user.upload(input("import-wxr"), WXR);
  await user.click(screen.getByRole("button", { name: "Advanced" }));
  const base = screen.getByRole("textbox", { name: "Category base" });
  await user.clear(base);
  await user.type(base, "../up");
  await user.click(screen.getByRole("button", { name: "Start import" }));
  expect(sent).toHaveLength(0);
  expect(base).toHaveAttribute("aria-invalid", "true");
  // Removing the export disables the start again.
  await user.click(screen.getByRole("button", { name: "Remove site-a.xml" }));
  expect(screen.getByRole("button", { name: "Start import" })).toBeDisabled();
});
