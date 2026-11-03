import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { type PermissionReportDto, type PluginDto, pluginsQuery } from "@/api/plugins";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { permission, plugin, report, trust, verifiedTrust } from "./fixtures";
import { InstallPluginDialog } from "./install-flow";
import { slotOf } from "./plugin-model";

interface Reply {
  status: number;
  body: unknown;
  type?: string;
  /** Never answers (a request still running). */
  hang?: boolean;
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
    if (reply.hang) return;
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

test("closing_during_install_refreshes_the_list", async () => {
  // The server may finish an install the closed dialog no longer waits for.
  const seen = stubRoutes({ "/api/v1/plugins": () => json({ items: [] }) });
  FakeXhr.replies.push({ status: 200, body: report() }, { status: 0, body: null, hang: true });
  function Harness() {
    const [open, setOpen] = useState(true);
    // The screen behind the dialog shows the list.
    useQuery(pluginsQuery);
    return <InstallPluginDialog open={open} onOpenChange={setOpen} onInstalled={() => {}} />;
  }
  renderWithRouter(<Harness />);
  await screen.findByRole("dialog", { name: "Install a plugin" });
  await pickBoth();
  await userEvent.click(screen.getByRole("button", { name: "Check permissions" }));
  await userEvent.click(await screen.findByRole("button", { name: "Approve and install" }));
  await waitFor(() => expect(FakeXhr.sent).toHaveLength(2));
  const lists = () => seen.filter((u) => u.pathname === "/api/v1/plugins").length;
  const before = lists();
  await userEvent.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(lists()).toBeGreaterThan(before));
});

// ---- M6-2: signed uploads --------------------------------------------------------------

test("install_sends_signature_parts", async () => {
  stubRoutes({ "/api/v1/plugins": () => json({ items: [] }) });
  FakeXhr.replies.push(
    { status: 200, body: report({ trust: verifiedTrust() }) },
    { status: 201, body: plugin({ enabled: false }) },
  );
  renderWithRouter(<InstallPluginDialog open onOpenChange={() => {}} onInstalled={() => {}} />);
  await screen.findByRole("dialog", { name: "Install a plugin" });
  // All four dropped at once are matched by name.
  const drop = document.querySelector<HTMLElement>("[data-slot=plugin-drop]");
  if (!drop) throw new Error("no drop zone");
  const files = [
    new File([MANIFEST], "ferrite-plugin.toml"),
    new File([new Uint8Array([0, 97, 115, 109])], "seo_helper.wasm"),
    new File(["untrusted comment: sig\n"], "ferrite-plugin.minisig"),
    new File(["untrusted comment: key\n"], "ferrite-plugin.pub"),
  ];
  fireEvent.drop(drop, { dataTransfer: { files, types: ["Files"] } });
  await userEvent.click(screen.getByRole("button", { name: "Check permissions" }));
  expect(await screen.findByText("Verified publisher")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Approve and install" }));
  await waitFor(() => expect(FakeXhr.sent).toHaveLength(2));
  for (const sent of FakeXhr.sent) {
    expect((sent.body.get("signature") as File).name).toBe("ferrite-plugin.minisig");
    expect((sent.body.get("publisher_key") as File).name).toBe("ferrite-plugin.pub");
  }
});

test("install_shows_publisher_refusal_from_422", async () => {
  stubRoutes({ "/api/v1/plugins": () => json({ items: [] }) });
  const refused = report({
    trust: trust({
      owner: null,
      refusal: {
        field: "publisher",
        message: "the data of `seo-helper` belongs to publisher FerriteCMS (registry)",
      },
    }),
  });
  FakeXhr.replies.push({
    status: 422,
    type: "application/problem+json",
    body: {
      type: "/api/v1/problems/validation",
      title: "Validation failed",
      status: 422,
      detail: "the data of `seo-helper` belongs to publisher FerriteCMS (registry)",
      errors: [
        {
          field: "publisher",
          detail: "the data of `seo-helper` belongs to publisher FerriteCMS (registry)",
        },
      ],
      report: refused,
    },
  });
  renderWithRouter(<InstallPluginDialog open onOpenChange={() => {}} onInstalled={() => {}} />);
  await screen.findByRole("dialog", { name: "Install a plugin" });
  await pickBoth();
  await userEvent.click(screen.getByRole("button", { name: "Check permissions" }));
  expect(await screen.findByText("This plugin cannot be installed")).toBeVisible();
  expect(screen.getAllByText(/belongs to publisher FerriteCMS/).length).toBeGreaterThan(0);
  expect(screen.getByRole("button", { name: "Approve and install" })).toBeDisabled();
});

test("dropped_files_are_matched_by_name", () => {
  expect(slotOf("ferrite-plugin.toml")).toBe("manifest");
  expect(slotOf("SEO.WASM")).toBe("component");
  expect(slotOf("ferrite-plugin.minisig")).toBe("signature");
  expect(slotOf("ferrite-plugin.pub")).toBe("key");
  expect(slotOf("readme.md")).toBeNull();
});
