// The terms sections: a hierarchical taxonomy is a checkbox tree (parents before their
// children, selection in the draft's order); a flat one offers creating a tag inline
// only with `manage_terms`.

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, describe, expect, test, vi } from "vitest";
import type { TaxonomyDto, TermDto } from "@/api/taxonomies";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { termTree, toggleTerm } from "./publishing";
import { TermsSection } from "./terms-section";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  if (!("scrollIntoView" in Element.prototype)) {
    Object.defineProperty(Element.prototype, "scrollIntoView", { value: () => {} });
  }
});

const CATEGORY: TaxonomyDto = {
  name: "category",
  label: "Categories",
  hierarchical: true,
  built_in: true,
  content_types: ["post"],
};
const TAG: TaxonomyDto = {
  name: "tag",
  label: "Tags",
  hierarchical: false,
  built_in: true,
  content_types: ["post"],
};

const term = (
  id: string,
  name: string,
  taxonomy: string,
  parent_id: string | null = null,
): TermDto => ({
  id,
  name,
  taxonomy,
  parent_id,
  slug: name.toLowerCase(),
  description: "",
});

const CATEGORIES = [
  term("c3", "Travel", "category"),
  term("c1", "News", "category"),
  term("c2", "Local", "category", "c1"),
  term("c4", "Europe", "category", "c3"),
];
const TAGS = [term("t1", "rust", "tag"), term("t2", "wasm", "tag")];

describe("terms", () => {
  test("tree_order_and_selection", async () => {
    stubRoutes({
      "/api/v1/taxonomies/category/terms": () => json({ items: CATEGORIES, next_cursor: null }),
    });
    const onChange = vi.fn();
    renderWithRouter(
      <TermsSection
        taxonomy={CATEGORY}
        selected={["t1"]}
        onChange={onChange}
        canCreate={false}
        disabled={false}
      />,
    );
    const group = await screen.findByRole("group", { name: "Categories" });
    // Parents first, children right under them, siblings by name.
    expect(
      within(group)
        .getAllByRole("checkbox")
        .map((c) => c.closest("label")?.textContent),
    ).toEqual(["News", "Local", "Travel", "Europe"]);
    const user = userEvent.setup();
    await user.click(within(group).getByRole("checkbox", { name: "Local" }));
    // Added after what is already chosen (any taxonomy's terms).
    expect(onChange).toHaveBeenLastCalledWith(["t1", "c2"]);
    // No inline creation without `manage_terms`.
    expect(screen.queryByRole("button", { name: /^New in/ })).toBeNull();
  });

  test("tree_unchecks_a_selected_term", async () => {
    stubRoutes({
      "/api/v1/taxonomies/category/terms": () => json({ items: CATEGORIES, next_cursor: null }),
    });
    const onChange = vi.fn();
    renderWithRouter(
      <TermsSection
        taxonomy={CATEGORY}
        selected={["c4", "c1"]}
        onChange={onChange}
        canCreate
        disabled={false}
      />,
    );
    const group = await screen.findByRole("group", { name: "Categories" });
    expect(within(group).getByRole("checkbox", { name: "Europe" })).toBeChecked();
    expect(within(group).getByRole("checkbox", { name: "Travel" })).not.toBeChecked();
    await userEvent.setup().click(within(group).getByRole("checkbox", { name: "Europe" }));
    expect(onChange).toHaveBeenLastCalledWith(["c1"]);
    expect(screen.getByRole("button", { name: "New in Categories" })).toBeVisible();
  });

  test("tag_creation_hidden_without_manage_terms", async () => {
    stubRoutes({ "/api/v1/taxonomies/tag/terms": () => json({ items: TAGS, next_cursor: null }) });
    renderWithRouter(
      <TermsSection
        taxonomy={TAG}
        selected={["t1"]}
        onChange={vi.fn()}
        canCreate={false}
        disabled={false}
      />,
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Add Tags…" }));
    await user.keyboard("brand-new");
    await waitFor(() => expect(screen.getByText("No matching terms")).toBeVisible());
    expect(screen.queryByRole("option", { name: /Create/ })).toBeNull();
  });

  test("tag_creation_with_manage_terms", async () => {
    const created = term("t9", "brand-new", "tag");
    const seen = stubRoutes({
      "/api/v1/taxonomies/tag/terms": (_url, req) =>
        req.method === "POST" ? json(created, 201) : json({ items: TAGS, next_cursor: null }),
    });
    const onChange = vi.fn();
    renderWithRouter(
      <TermsSection
        taxonomy={TAG}
        selected={["t1"]}
        onChange={onChange}
        canCreate
        disabled={false}
      />,
    );
    // The chosen tag is a chip; the picker offers only the others.
    expect(await screen.findByRole("button", { name: "Remove rust" })).toBeVisible();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Add Tags…" }));
    expect(screen.getByRole("option", { name: "wasm" })).toBeVisible();
    expect(screen.queryByRole("option", { name: "rust" })).toBeNull();
    await user.keyboard("brand-new");
    await user.click(await screen.findByRole("option", { name: "Create “brand-new”" }));
    await waitFor(() => expect(onChange).toHaveBeenLastCalledWith(["t1", "t9"]));
    expect(seen.some((u) => u.pathname === "/api/v1/taxonomies/tag/terms")).toBe(true);
  });

  test("disabled_sections_change_nothing", async () => {
    stubRoutes({ "/api/v1/taxonomies/tag/terms": () => json({ items: TAGS, next_cursor: null }) });
    renderWithRouter(
      <TermsSection taxonomy={TAG} selected={["t1"]} onChange={vi.fn()} canCreate disabled />,
    );
    await screen.findByText("rust");
    expect(screen.queryByRole("button", { name: "Remove rust" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add Tags…" })).toBeNull();
  });

  test("tree_helpers", () => {
    // A cycle is cut and nothing is lost.
    const cyclic = [term("a", "A", "c", "b"), term("b", "B", "c", "a")];
    expect(
      termTree(cyclic)
        .map((n) => n.term.id)
        .sort(),
    ).toEqual(["a", "b"]);
    expect(toggleTerm(["a", "b"], "a")).toEqual(["b"]);
    expect(toggleTerm(["b"], "a")).toEqual(["b", "a"]);
  });
});
