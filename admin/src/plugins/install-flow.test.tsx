import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { PermissionReportDto, PluginDto } from "@/api/plugins";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { permission, plugin, report } from "./fixtures";
import { InstallPluginDialog } from "./install-flow";

interface Reply {
  status: number;
  body: unknown;
  type?: string;
}

/** An `XMLHttpRequest` that answers each `send` with the next queued reply. */
class FakeXhr {
  static replies: Reply[] = [];
  static sent: { url: string; body: FormData }[] = [];
  upload = { onprogress: null as ((e: ProgressEvent) => void) | null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  withCredentials = false;
  status = 0;
  statusText = "";
  responseText = "";
  private url = "";
  private type = "application/json";
  open(_method: string, url: string) {
    this.url = url;
  }
  setRequestHeader() {}
  getResponseHeader(name: string) {
    return name.toLowerCase() === "content-type" ? this.type : null;
  }
  send(body: FormData) {
    FakeXhr.sent.push({ url: this.url, body });
    const reply = FakeXhr.replies.shift();
    if (!reply) throw new Error(`no reply queued for ${this.url}`);
    setTimeout(() => {
      this.status = reply.status;
      this.responseText = JSON.stringify(reply.body);
      this.type = reply.type ?? "application/json";
      this.onload?.();
    }, 0);
  }
  abort() {
    this.onabort?.();
  }
}

beforeEach(() => {
  FakeXhr.replies = [];
  FakeXhr.sent = [];
  vi.stubGlobal("XMLHttpRequest", FakeXhr);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const MANIFEST = '[plugin]\nname = "seo-helper"\nversion = "1.2.0"\n';

function input(slot: string): HTMLInputElement {
  const el = document.querySelector<HTMLInputElement>(`[data-slot=${slot}]`);
  if (!el) throw new Error(`no ${slot}`);
  return el;
}

async function pickBoth() {
  await userEvent.upload(
    input("plugin-manifest-input"),
    new File([MANIFEST], "ferrite-plugin.toml", { type: "application/toml" }),
  );
  await userEvent.upload(
    input("plugin-component-input"),
    new File([new Uint8Array([0, 97, 115, 109])], "seo_helper.wasm", { type: "application/wasm" }),
  );
}

test("fingerprint_mismatch_shows_the_new_report", async () => {
  stubRoutes({ "/api/v1/plugins": () => json({ items: [] }) });
  const first: PermissionReportDto = report({ fingerprint: "1".repeat(64) });
  const second: PermissionReportDto = report({
    fingerprint: "2".repeat(64),
    artifact: "b".repeat(64),
    items: [
      permission(),
      permission({
        key: "mail",
        text: "send e-mail to the site's administrators",
        risk: "high",
        high_risk: true,
      }),
    ],
    added: [
      permission(),
      permission({
        key: "mail",
        text: "send e-mail to the site's administrators",
        risk: "high",
        high_risk: true,
      }),
    ],
  });
  const installed: PluginDto = plugin({ enabled: false });
  const onInstalled = vi.fn();
  FakeXhr.replies.push(
    { status: 200, body: first },
    {
      status: 422,
      type: "application/problem+json",
      body: {
        type: "/api/v1/problems/validation",
        title: "Validation failed",
        status: 422,
        detail: "the upload does not match the approved report",
        errors: [{ field: "approve", detail: "does not match" }],
        report: second,
      },
    },
    { status: 201, body: installed },
  );
  renderWithRouter(<InstallPluginDialog open onOpenChange={() => {}} onInstalled={onInstalled} />);
  await screen.findByRole("dialog", { name: "Install a plugin" });
  await pickBoth();
  await userEvent.click(screen.getByRole("button", { name: "Check permissions" }));

  // The first report, then approve it.
  const facts = await screen.findByRole("group", { name: "What is being installed" });
  expect(facts).toHaveTextContent("a".repeat(64));
  await userEvent.click(screen.getByRole("button", { name: "Approve and install" }));

  // The files changed: the server's new report replaces the old one and asks again.
  const changed = await screen.findByText(/The files changed after the check/);
  expect(changed).toBeVisible();
  expect(screen.getByRole("group", { name: "What is being installed" })).toHaveTextContent(
    "b".repeat(64),
  );
  const items = screen.getAllByRole("listitem").filter((li) => li.dataset.slot === "permission");
  expect(items[0]).toHaveAttribute("data-key", "mail");
  expect(onInstalled).not.toHaveBeenCalled();

  // Approving again sends the new fingerprint.
  await userEvent.click(screen.getByRole("button", { name: "Approve and install" }));
  await waitFor(() => expect(FakeXhr.sent).toHaveLength(3));
  expect(FakeXhr.sent.map((s) => s.url)).toEqual([
    "/api/v1/plugins/inspect",
    "/api/v1/plugins",
    "/api/v1/plugins",
  ]);
  expect(FakeXhr.sent[1]?.body.get("approve")).toBe("1".repeat(64));
  expect(FakeXhr.sent[2]?.body.get("approve")).toBe("2".repeat(64));
  // Both files went with each request.
  expect((FakeXhr.sent[2]?.body.get("manifest") as File).name).toBe("ferrite-plugin.toml");
  expect((FakeXhr.sent[2]?.body.get("component") as File).name).toBe("seo_helper.wasm");

  // Installed disabled, with Enable.
  const done = await screen.findByText(/is installed and disabled/);
  expect(done).toHaveTextContent("seo-helper 1.2.0 is installed and disabled");
  expect(screen.getByRole("button", { name: "Enable" })).toBeVisible();
  expect(onInstalled).toHaveBeenCalledWith(installed);
});

test("update_says_it_replaces_the_installed_version", async () => {
  stubRoutes({ "/api/v1/plugins": () => json({ items: [plugin({ version: "1.1.0" })] }) });
  FakeXhr.replies.push({ status: 200, body: report({ update: true, added: [] }) });
  renderWithRouter(<InstallPluginDialog open onOpenChange={() => {}} onInstalled={() => {}} />);
  await screen.findByRole("dialog", { name: "Install a plugin" });
  await pickBoth();
  await userEvent.click(screen.getByRole("button", { name: "Check permissions" }));
  const note = await screen.findByText(/updates the installed seo-helper/);
  expect(note).toHaveTextContent("This updates the installed seo-helper (version 1.1.0)");
  expect(screen.getByText("It asks for nothing new.")).toBeVisible();
});

test("refused_inspect_shows_the_server_problems", async () => {
  stubRoutes({ "/api/v1/plugins": () => json({ items: [] }) });
  FakeXhr.replies.push({
    status: 422,
    type: "application/problem+json",
    body: {
      type: "/api/v1/problems/validation",
      title: "Validation failed",
      status: 422,
      detail: "the manifest is not valid",
      errors: [{ field: "plugin.name", detail: "must match [a-z0-9-]" }],
    },
  });
  renderWithRouter(<InstallPluginDialog open onOpenChange={() => {}} onInstalled={() => {}} />);
  const dialog = await screen.findByRole("dialog", { name: "Install a plugin" });
  await pickBoth();
  await userEvent.click(screen.getByRole("button", { name: "Check permissions" }));
  const alert = await within(dialog).findByRole("alert");
  expect(alert).toHaveTextContent("plugin.name: must match [a-z0-9-]");
});
