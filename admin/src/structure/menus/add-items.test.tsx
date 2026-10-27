import { QueryClient } from "@tanstack/react-query";
import { screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { qk } from "@/api/keys";
import { renderWithRouter, stubRoutes } from "@/test/render";
import { AddItems } from "./add-items";

const ME = {
  id: "0190f1c0-0000-7000-8000-0000000000bb",
  login: "menus",
  email: "menus@example.test",
  display_name: "Menus",
  role: "custom",
  must_reset: false,
  totp_enabled: false,
  // Menus only: the sidebar shows no content type.
  capabilities: ["manage_menus"],
  scopes: [] as string[],
};
const TYPES = [
  { name: "post", label: "Posts", built_in: true, hierarchical: false, has_archive: true },
  { name: "page", label: "Pages", built_in: true, hierarchical: true, has_archive: false },
];

afterEach(() => {
  vi.unstubAllGlobals();
});

test("content_tab_without_nav_types_says_so", async () => {
  stubRoutes({});
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  qc.setQueryData(qk.me, ME);
  qc.setQueryData(qk.types, TYPES);
  renderWithRouter(<AddItems onAdd={() => undefined} room={10} disabled={false} />, qc);
  // Not a skeleton that never ends.
  expect(
    await screen.findByText("No content types to link to. Use a custom link instead."),
  ).toBeInTheDocument();
  expect(screen.queryByRole("status", { name: "Recently updated" })).toBeNull();
});
