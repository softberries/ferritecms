import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { PublisherKeyList, RegistryDto } from "@/api/plugins";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { installedTrust, plugin, report, verifiedTrust } from "./fixtures";
import { PluginDetail } from "./plugin-detail";
import { PublishersScreen } from "./publishers";
import { RegistryBrowser } from "./registry-browser";

const META = {
  url: "https://example.dev/registry/v1/index.json",
  name: "Test registry",
  serial: 3,
  generated_at: "2026-11-20T10:00:00Z",
  checked_at: "2026-11-21T10:00:00Z",
  last_error: null,
  last_error_at: null,
};

function registry(over: Partial<RegistryDto> = {}): RegistryDto {
  return {
    registry: META,
    plugins: [
      {
        name: "seo",
        publisher: { id: "ferritecms.dev", name: "FerriteCMS" },
        description: "Meta tags <b>and</b> Open Graph",
        homepage: null,
        license: "AGPL-3.0-only",
        latest: "0.3.0",
        versions: [{ version: "0.3.0", world: "w", published_at: null, yanked: null }],
        installed: null,
      },
    ],
    ...over,
  };
}

test("registry_browser_search_and_empty_states", async () => {
  const seen = stubRoutes({
    "/api/v1/plugins/registry": (url) =>
      url.searchParams.get("q") === "zzz" ? json(registry({ plugins: [] })) : json(registry()),
  });
  renderWithRouter(<RegistryBrowser />);
  const list = await screen.findByRole("list", { name: "Registry plugins" });
  expect(within(list).getByText("seo")).toBeVisible();
  expect(within(list).getByText("FerriteCMS · Verified")).toBeVisible();
  // The index's text is text.
  expect(within(list).getByText("Meta tags <b>and</b> Open Graph")).toBeVisible();
  expect(within(list).getByRole("button", { name: "Install seo" })).toBeVisible();
  await userEvent.type(screen.getByRole("searchbox", { name: "Search the registry" }), "zzz");
  expect(await screen.findByText("No plugins match")).toBeVisible();
  expect(seen.some((u) => u.searchParams.get("q") === "zzz")).toBe(true);
});

test("registry_browser_without_registry", async () => {
  stubRoutes({ "/api/v1/plugins/registry": () => json({ registry: null, plugins: [] }) });
  renderWithRouter(<RegistryBrowser />);
  expect(await screen.findByText("No plugin registry")).toBeVisible();
});

test("registry_browser_not_checked_yet", async () => {
  stubRoutes({
    "/api/v1/plugins/registry": () =>
      json({ registry: { ...META, name: null, serial: null, checked_at: null }, plugins: [] }),
  });
  renderWithRouter(<RegistryBrowser />);
  expect(await screen.findByText("The registry has not been checked yet")).toBeVisible();
});

test("registry_install_uses_report_step", async () => {
  const bodies: unknown[] = [];
  stubRoutes({
    "/api/v1/plugins/registry": () => json(registry()),
    "/api/v1/plugins": () => json({ items: [] }),
    "/api/v1/plugins/registry/inspect": async (_u, req) => {
      bodies.push(await req.json());
      return json(report({ plugin: "seo", version: "0.3.0", trust: verifiedTrust() }));
    },
    "/api/v1/plugins/registry/install": async (_u, req) => {
      bodies.push(await req.json());
      return json(plugin({ name: "seo", version: "0.3.0", enabled: false }), 201);
    },
  });
  renderWithRouter(<RegistryBrowser />);
  await userEvent.click(await screen.findByRole("button", { name: "Install seo" }));
  const dialog = await screen.findByRole("dialog", { name: "Install seo from the registry" });
  expect(await within(dialog).findByText("Verified publisher")).toBeVisible();
  await userEvent.click(within(dialog).getByRole("button", { name: "Approve and install" }));
  expect(await within(dialog).findByText(/is installed and disabled/)).toBeVisible();
  expect(bodies[0]).toEqual({ name: "seo", version: "0.3.0" });
  expect(bodies[1]).toEqual({ name: "seo", version: "0.3.0", approve: "f".repeat(64) });
});

test("publishers_revoke_dialog_requires_reason", async () => {
  const list: PublisherKeyList = {
    policy: "any",
    items: [
      {
        key_id: "5A1C3D0000000000",
        public_key: "RWQ",
        label: "example.dev",
        source: "approved",
        registry_publisher: null,
        trusted_at: "2026-11-01T10:00:00Z",
        revoked_at: null,
        revoked_reason: null,
        compromised: false,
        plugins: ["seo"],
      },
    ],
  };
  const sent: unknown[] = [];
  stubRoutes({
    "/api/v1/plugins/publishers": () => json(list),
    "/api/v1/plugins/publishers/5A1C3D0000000000/revoke": async (_u, req) => {
      sent.push(await req.json());
      return json({
        ...list.items[0],
        revoked_at: "2026-11-02T10:00:00Z",
        revoked_reason: "stolen",
        compromised: true,
      });
    },
  });
  renderWithRouter(<PublishersScreen />);
  expect(await screen.findByText(/Signature policy: any/)).toBeVisible();
  await userEvent.click(await screen.findByRole("button", { name: "Revoke key 5A1C3D0000000000" }));
  const dialog = await screen.findByRole("dialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Revoke key" }));
  expect(await within(dialog).findByText("Give a reason.")).toBeVisible();
  expect(sent).toHaveLength(0);
  await userEvent.type(within(dialog).getByLabelText("Reason"), "stolen");
  await userEvent.click(
    within(dialog).getByLabelText("The key was stolen or leaked (disables its plugins)"),
  );
  await userEvent.click(within(dialog).getByRole("button", { name: "Revoke key" }));
  await waitFor(() => expect(sent).toEqual([{ reason: "stolen", compromised: true }]));
});

test("detail_shows_publisher_and_update", async () => {
  stubRoutes({
    "/api/v1/plugins/seo": () =>
      json(
        plugin({
          name: "seo",
          version: "0.2.0",
          trust: installedTrust({
            state: "verified",
            owner: { id: "reg:9F1E000000000000:ferritecms.dev", label: "FerriteCMS (registry)" },
            signer: {
              key_id: "432051207B9BA065",
              source: "registry",
              publisher: { id: "ferritecms.dev", name: "FerriteCMS" },
              claimed: "ferritecms.dev",
            },
            source: { kind: "registry", url: META.url, version: null },
            update: { version: "0.3.0", publisher_matches: true },
          }),
        }),
      ),
    "/api/v1/plugins/seo/metrics": () =>
      json({
        window_secs: 86_400,
        failures: [],
        node: { id: "x", since: META.checked_at, hooks: [] },
      }),
    "/api/v1/plugins/seo/settings": () =>
      json({ schema: [], values: {}, secrets: {}, invalid: [] }),
  });
  renderWithRouter(<PluginDetail name="seo" />);
  const section = await screen.findByText("432051207B9BA065");
  expect(section).toBeVisible();
  expect(screen.getAllByText("FerriteCMS · Verified").length).toBeGreaterThan(0);
  expect(screen.getByText("FerriteCMS (registry)")).toBeVisible();
  expect(screen.getByText("Installed from the registry")).toBeVisible();
  expect(screen.getByRole("button", { name: "Update seo to 0.3.0" })).toBeVisible();
});
