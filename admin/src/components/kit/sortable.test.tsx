import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PencilIcon } from "lucide-react";
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
    // A tree row starts one 24 px indent further right per level, as on screen.
    const level = Number(row.getAttribute("aria-level") ?? "1");
    return new DOMRect((level - 1) * 24, rows.indexOf(row) * ROW_H, 320, ROW_H);
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

  it("sortable_tree_keyboard_drag_keeps_the_level", async () => {
    // Home, About (Team, History), Blog. Home goes down past the nested rows, which start
    // 24 px further right; the keyboard only moves rows up and down, so Home stays a root.
    const nodes: TreeNode<string>[] = [
      { id: "n1", parentId: null, data: "Home" },
      { id: "n2", parentId: null, data: "About" },
      { id: "n3", parentId: "n2", data: "Team" },
      { id: "n4", parentId: "n2", data: "History" },
      { id: "n5", parentId: null, data: "Blog" },
    ];
    const onChange = vi.fn<(next: TreeNode<string>[]) => void>();
    const user = userEvent.setup();
    render(
      <SortableTree
        nodes={nodes}
        label={(d) => d}
        render={(n) => <span>{n.data}</span>}
        onChange={onChange}
        maxDepth={2}
      />,
    );
    screen.getByRole("button", { name: "Reorder Home" }).focus();
    await user.keyboard("[Space]");
    await tick();
    for (let i = 0; i < 3; i++) {
      await user.keyboard("[ArrowDown]");
      await tick();
    }
    await user.keyboard("[Space]");
    await tick();
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0]?.[0] ?? [];
    // Between History and Blog, at the top level.
    expect(next.find((n) => n.id === "n1")?.parentId).toBeNull();
    expect(next.filter((n) => n.parentId === null).map((n) => n.data)).toEqual([
      "About",
      "Home",
      "Blog",
    ]);
  });

  it("sortable_tree_pointer_indent_holds_while_the_pointer_rests", async () => {
    // CI run 40404431115: the dragged row is drawn at its projected level by its own
    // inline margin, which moves its rect; dnd-kit takes that layout shift off its drag
    // delta, so the level fell back, the margin with it, and the next frame indented it
    // again: the guide flickered every frame and the drop's level was the frame's. Here
    // a row's rect follows its frame's margin, as in a browser.
    vi.restoreAllMocks();
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (
      this: Element,
    ) {
      const row = this.closest("[data-sortable-row]");
      if (!row?.parentElement) return new DOMRect(0, 0, 0, 0);
      const rows = Array.from(row.parentElement.querySelectorAll(":scope > [data-sortable-row]"));
      const frame = row.firstElementChild as HTMLElement | null;
      const left = Number.parseFloat(frame?.style.marginInlineStart || "0");
      return new DOMRect(left, rows.indexOf(row) * ROW_H, 320 - left, ROW_H);
    });
    // The browser reports the narrower row (dnd-kit then measures it again); here every
    // observer hears after each render.
    const observers = new Set<ResizeObserverCallback>();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(private cb: ResizeObserverCallback) {}
        observe() {
          observers.add(this.cb);
        }
        unobserve() {}
        disconnect() {
          observers.delete(this.cb);
        }
      },
    );
    const settle = async () => {
      await tick();
      act(() => {
        for (const cb of observers) cb([], {} as ResizeObserver);
      });
      await tick();
    };
    const nodes: TreeNode<string>[] = [
      { id: "n1", parentId: null, data: "Alpha" },
      { id: "n2", parentId: null, data: "Beta" },
    ];
    const onChange = vi.fn<(next: TreeNode<string>[], moved: { id: string }) => void>();
    render(
      <SortableTree
        nodes={nodes}
        label={(d) => d}
        render={(n) => <span>{n.data}</span>}
        onChange={onChange}
        maxDepth={8}
        unordered
      />,
    );
    const handle = screen.getByRole("button", { name: "Reorder Beta" });
    const at = (x: number) => ({ clientX: x, clientY: 60, isPrimary: true, button: 0 });
    const guide = () =>
      document.querySelector<HTMLElement>("[data-placeholder]")?.style.marginInlineStart;
    const margins: (string | undefined)[] = [];
    fireEvent.pointerDown(handle, at(10));
    try {
      for (const x of [16, 25, 40]) {
        fireEvent.pointerMove(document, at(x));
        await settle();
      }
      // Several renders with the pointer at rest: the guide stays one level in.
      for (let i = 0; i < 4; i++) {
        await settle();
        margins.push(guide());
      }
    } finally {
      fireEvent.pointerUp(document, at(40));
      // dnd-kit stops clicks on the document for 50 ms after a pointer drag: wait it out,
      // or it eats the next test's first clicks.
      await act(() => new Promise((r) => setTimeout(r, 60)));
      vi.unstubAllGlobals();
    }
    expect(margins).toEqual(["24px", "24px", "24px", "24px"]);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0]?.[1]).toMatchObject({ id: "n2", parentId: "n1" });
  });

  it("sortable_tree_unordered_moves_only_change_the_parent", async () => {
    // Terms (P35): siblings have no order, so a drop that keeps the parent is no move, and
    // the menu is the caller's own.
    const nodes: TreeNode<string>[] = [
      { id: "n1", parentId: null, data: "Apples" },
      { id: "n2", parentId: null, data: "Fruit" },
      { id: "n3", parentId: "n2", data: "Kiwi" },
      { id: "n4", parentId: null, data: "Pears" },
    ];
    const onChange = vi.fn<(next: TreeNode<string>[], moved: { id: string }) => void>();
    const edit = vi.fn<(id: string) => void>();
    const moveTo = vi.fn<(id: string) => void>();
    const user = userEvent.setup();
    render(
      <SortableTree
        nodes={nodes}
        label={(d) => d}
        render={(n) => <span>{n.data}</span>}
        onChange={onChange}
        maxDepth={8}
        unordered
        actions={(node) => [
          {
            key: "edit",
            label: "Edit",
            icon: PencilIcon,
            disabled: false,
            run: () => edit(node.id),
          },
          {
            key: "move",
            label: "Move to…",
            icon: PencilIcon,
            disabled: false,
            overlay: true,
            run: () => moveTo(node.id),
          },
        ]}
      />,
    );
    // The caller's menu replaces Move up / Move down / Indent / Outdent.
    await user.click(screen.getByRole("button", { name: "Actions for Pears" }));
    const menu = await screen.findByRole("menu");
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent),
    ).toEqual(["Edit", "Move to…"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Edit" }));
    expect(edit).toHaveBeenCalledWith("n4");
    // An overlay action runs once the menu has closed (so the overlay gets focus).
    await user.click(screen.getByRole("button", { name: "Actions for Pears" }));
    const again = await screen.findByRole("menu");
    await user.click(within(again).getByRole("menuitem", { name: "Move to…" }));
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await waitFor(() => expect(moveTo).toHaveBeenCalledWith("n4"));

    // Pears up past Kiwi, still at the top level: only the order would change, no move.
    screen.getByRole("button", { name: "Reorder Pears" }).focus();
    await user.keyboard("[Space]");
    await tick();
    await user.keyboard("[ArrowUp]");
    await tick();
    await user.keyboard("[ArrowUp]");
    await tick();
    await user.keyboard("[Space]");
    await tick();
    expect(onChange).not.toHaveBeenCalled();

    // Apples down onto Fruit's first child: it lands under Fruit.
    screen.getByRole("button", { name: "Reorder Apples" }).focus();
    await user.keyboard("[Space]");
    await tick();
    await user.keyboard("[ArrowDown]");
    await tick();
    await user.keyboard("[Space]");
    await tick();
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0]?.[1]).toMatchObject({ id: "n1", parentId: "n2" });
    expect(document.querySelector("[data-slot=sortable-status]")).toHaveTextContent(
      "Apples is now under Fruit.",
    );
  });

  it("sortable_tree_leading_and_trailing_actions_wrap_the_moves", async () => {
    // Menus: Edit before Move up / Move down / Indent / Outdent, Remove after them.
    const nodes: TreeNode<string>[] = [
      { id: "n1", parentId: null, data: "Home" },
      { id: "n2", parentId: null, data: "Blog" },
    ];
    const edit = vi.fn<(id: string) => void>();
    const remove = vi.fn<(id: string) => void>();
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(
      <SortableTree
        nodes={nodes}
        label={(d) => d}
        render={(n) => <span>{n.data}</span>}
        onChange={onChange}
        maxDepth={2}
        leadingActions={(node) => [
          {
            key: "edit",
            label: "Edit",
            icon: PencilIcon,
            disabled: false,
            run: () => edit(node.id),
          },
        ]}
        trailingActions={(node) => [
          {
            key: "remove",
            label: "Remove",
            icon: PencilIcon,
            disabled: false,
            separated: true,
            run: () => remove(node.id),
          },
        ]}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Actions for Blog" }));
    const menu = await screen.findByRole("menu");
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent),
    ).toEqual(["Edit", "Move up", "Move down", "Indent", "Outdent", "Remove"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Remove" }));
    expect(remove).toHaveBeenCalledWith("n2");
    // The moves still work beside them.
    await user.click(screen.getByRole("button", { name: "Actions for Blog" }));
    await user.click(
      within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Indent" }),
    );
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0]?.[1]).toMatchObject({ id: "n2", parentId: "n1" });
    expect(edit).not.toHaveBeenCalled();
  });
});
