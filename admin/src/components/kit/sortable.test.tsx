import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SortableList } from "./sortable-list";
import { SortableTree } from "./sortable-tree";
import type { TreeNode } from "./tree-model";

interface Item {
  id: string;
  name: string;
}

const ITEMS: Item[] = [
  { id: "id-alpha", name: "Alpha" },
  { id: "id-beta", name: "Beta" },
  { id: "id-gamma", name: "Gamma" },
];

const ROW_H = 40;

// jsdom lays nothing out, so every rect is zero and dnd-kit's keyboard coordinates find
// no neighbour. Give each sortable row a 40 px slot by its position in the list, and
// everything inside a row its row's rect.
function stubLayout() {
  const rectOf = (el: Element): DOMRect => {
    const row = el.closest("[data-sortable-row]");
    if (!row?.parentElement) return new DOMRect(0, 0, 0, 0);
    const rows = Array.from(row.parentElement.querySelectorAll(":scope > [data-sortable-row]"));
    return new DOMRect(0, rows.indexOf(row) * ROW_H, 320, ROW_H);
  };
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    return rectOf(this);
  });
}

beforeEach(stubLayout);
afterEach(() => {
  vi.restoreAllMocks();
});

const tick = () => act(() => new Promise((r) => setTimeout(r, 0)));

describe("sortable", () => {
  it("sortable_list_keyboard_reorder", async () => {
    const onReorder = vi.fn<(next: Item[]) => void>();
    const user = userEvent.setup();
    render(
      <SortableList
        items={ITEMS}
        getId={(i) => i.id}
        label={(i) => i.name}
        render={(i) => <span>{i.name}</span>}
        onReorder={onReorder}
      />,
    );
    const handle = screen.getByRole("button", { name: "Reorder Beta" });
    handle.focus();
    await user.keyboard("[Space]");
    await tick();
    // dnd-kit's live region speaks the label, never the raw id.
    await waitFor(() => {
      expect(document.body).toHaveTextContent(/Picked up Beta/);
    });
    expect(document.body).not.toHaveTextContent(/id-beta/);
    await user.keyboard("[ArrowDown]");
    await tick();
    await user.keyboard("[Space]");
    await tick();
    expect(onReorder).toHaveBeenCalledTimes(1);
    expect(onReorder.mock.calls[0]?.[0].map((i) => i.name)).toEqual(["Alpha", "Gamma", "Beta"]);
    await waitFor(() => {
      expect(document.body).toHaveTextContent(/Beta dropped at position 3 of 3/);
    });
  });

  it("sortable_list_menu_moves", async () => {
    const onReorder = vi.fn<(next: Item[]) => void>();
    const user = userEvent.setup();
    render(
      <SortableList
        items={ITEMS}
        getId={(i) => i.id}
        label={(i) => i.name}
        render={(i) => <span>{i.name}</span>}
        onReorder={onReorder}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Actions for Alpha" }));
    const menu = await screen.findByRole("menu");
    // The first row cannot go up.
    expect(within(menu).getByRole("menuitem", { name: "Move up" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await user.click(within(menu).getByRole("menuitem", { name: "Move down" }));
    expect(onReorder).toHaveBeenCalledTimes(1);
    expect(onReorder.mock.calls[0]?.[0].map((i) => i.name)).toEqual(["Beta", "Alpha", "Gamma"]);
    // Said out loud as well, by label.
    expect(document.querySelector("[data-slot=sortable-status]")).toHaveTextContent(
      "Alpha moved to position 2 of 3.",
    );
  });

  it("sortable_tree_menu_moves", async () => {
    // Home, About (Team), Blog
    const nodes: TreeNode<string>[] = [
      { id: "n1", parentId: null, data: "Home" },
      { id: "n2", parentId: null, data: "About" },
      { id: "n3", parentId: "n2", data: "Team" },
      { id: "n4", parentId: null, data: "Blog" },
    ];
    const onChange = vi.fn<(next: TreeNode<string>[]) => void>();
    const user = userEvent.setup();
    const { rerender } = render(
      <SortableTree
        nodes={nodes}
        label={(d) => d}
        render={(n) => <span>{n.data}</span>}
        onChange={onChange}
        maxDepth={1}
      />,
    );
    // Rows carry their level for assistive technology.
    expect(screen.getByRole("button", { name: "Reorder Team" }).closest("li")).toHaveAttribute(
      "aria-level",
      "2",
    );

    // Blog: indent under About (as its last child).
    await user.click(screen.getByRole("button", { name: "Actions for Blog" }));
    let menu = await screen.findByRole("menu");
    await user.click(within(menu).getByRole("menuitem", { name: "Indent" }));
    expect(onChange).toHaveBeenCalledTimes(1);
    const indented = onChange.mock.calls[0]?.[0] ?? [];
    expect(indented.find((n) => n.id === "n4")?.parentId).toBe("n2");
    expect(document.querySelector("[data-slot=sortable-status]")).toHaveTextContent(
      "Blog is now under About, position 2 of 2.",
    );

    // Team is at maxDepth 1 and has no previous sibling: Indent is off; Outdent works.
    rerender(
      <SortableTree
        nodes={indented}
        label={(d) => d}
        render={(n) => <span>{n.data}</span>}
        onChange={onChange}
        maxDepth={1}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Actions for Blog" }));
    menu = await screen.findByRole("menu");
    // Blog's previous sibling is Team, but Blog is already at maxDepth.
    expect(within(menu).getByRole("menuitem", { name: "Indent" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await user.click(within(menu).getByRole("menuitem", { name: "Move up" }));
    const moved = onChange.mock.calls[1]?.[0] ?? [];
    expect(moved.filter((n) => n.parentId === "n2").map((n) => n.data)).toEqual(["Blog", "Team"]);
  });
});
