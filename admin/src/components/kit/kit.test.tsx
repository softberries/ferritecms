import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { CONTENT_STATUSES } from "@/api/content";
import { DataTable } from "./data-table";
import { dataTableColumns, type RowSelectionState } from "./data-table-features";
import { MetricTile } from "./metric-tile";
import { SaveState } from "./save-state";
import { StatusBadge } from "./status-badge";

describe("kit", () => {
  it("status_badge_labels_every_status", () => {
    const labels = {
      draft: "Draft",
      pending: "Pending review",
      scheduled: "Scheduled",
      published: "Published",
      private: "Private",
      trash: "Trash",
    } as const;
    for (const status of CONTENT_STATUSES) {
      const { container, unmount } = render(<StatusBadge status={status} />);
      const badge = container.querySelector("[data-slot=status-badge]");
      // A label, not colour alone; the dot is hidden from assistive technology.
      expect(badge).toHaveTextContent(labels[status]);
      expect(badge?.querySelector("[aria-hidden=true]")).not.toBeNull();
      expect(badge?.className).toContain(`text-status-${status}`);
      unmount();
    }
  });

  it("data_table_keyboard_selection", async () => {
    interface Item {
      id: string;
      title: string;
    }
    const items: Item[] = [
      { id: "a", title: "Alpha" },
      { id: "b", title: "Beta" },
      { id: "c", title: "Gamma" },
    ];
    const col = dataTableColumns<Item>();
    const columns = col.columns([col.accessor("title", { header: "Title" })]);
    const opened = vi.fn<(item: Item) => void>();
    const seen: RowSelectionState[] = [];

    function Harness() {
      const [state, setState] = useState<RowSelectionState>({});
      return (
        <DataTable
          label="Items"
          columns={columns}
          data={items}
          getRowId={(r) => r.id}
          selection={{
            state,
            onChange: (next) => {
              seen.push(next);
              setState(next);
            },
          }}
          onRowOpen={opened}
          loading={false}
          empty={<p>Nothing</p>}
        />
      );
    }

    const user = userEvent.setup();
    render(<Harness />);
    const table = screen.getByRole("table", { name: "Items" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    // Roving tabindex: only the active row is in the tab order.
    expect(rows.map((r) => r.tabIndex)).toEqual([0, -1, -1]);

    rows[0]?.focus();
    await user.keyboard("j");
    expect(rows[1]).toHaveFocus();
    await user.keyboard("j");
    expect(rows[2]).toHaveFocus();
    await user.keyboard("j"); // stays on the last row
    expect(rows[2]).toHaveFocus();
    await user.keyboard("k");
    expect(rows[1]).toHaveFocus();

    await user.keyboard("x");
    expect(seen.at(-1)).toEqual({ b: true });
    expect(rows[1]).toHaveAttribute("data-state", "selected");
    expect(within(rows[1] as HTMLElement).getByRole("checkbox")).toBeChecked();
    await user.keyboard("x");
    expect(seen.at(-1)).toEqual({});

    await user.keyboard("{Enter}");
    expect(opened).toHaveBeenCalledTimes(1);
    expect(opened).toHaveBeenCalledWith(items[1]);
  });

  it("save_state_announces", () => {
    const { rerender } = render(<SaveState state="saving" />);
    const region = screen.getByRole("status");
    expect(region).toHaveAttribute("aria-live", "polite");
    expect(region).toHaveTextContent("Saving…");
    rerender(<SaveState state="saved" at={new Date(2026, 9, 25, 14, 2)} />);
    expect(screen.getByRole("status")).toBe(region);
    expect(region).toHaveTextContent("Saved at 2:02 PM");
    rerender(<SaveState state="error" />);
    expect(region).toHaveTextContent("Couldn't save");
  });

  it("metric_tile_shows_text_not_only_chart", () => {
    const { container } = render(
      <MetricTile label="Published posts" value={1284} delta={-12} series={[3, 5, 4, 8, 6]} />,
    );
    expect(screen.getByText("Published posts")).toBeInTheDocument();
    expect(screen.getByText("1,284")).toBeInTheDocument();
    // The change is spoken in words, not only shown as an arrow.
    expect(screen.getByText("Down 12 on the previous period")).toBeInTheDocument();
    // The chart is decorative.
    const spark = container.querySelector("[data-slot=sparkline]");
    expect(spark).toHaveAttribute("aria-hidden", "true");
  });
});
