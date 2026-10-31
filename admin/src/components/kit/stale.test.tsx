// Stale states (M4-4 R-c3, as the palette's P81): while a new filter, page or range loads,
// the previous rows and figures stay at full colour; the container is `aria-busy` and a
// pulsing ember line (`StaleBar`) says it. Dimmed to 60 %, muted text fell to about 2:1.

import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import type { MediaDto } from "@/api/media";
import { Panel } from "@/dashboard/panel";
import { MediaGrid } from "@/media/media-grid";
import { DataTable } from "./data-table";
import { dataTableColumns } from "./data-table-features";

interface Item {
  id: string;
  title: string;
}
const col = dataTableColumns<Item>();
const columns = col.columns([col.accessor("title", { header: "Title" })]);

function table(stale: boolean) {
  return (
    <DataTable
      label="Items"
      columns={columns}
      data={[{ id: "a", title: "Alpha" }]}
      getRowId={(r) => r.id}
      loading={false}
      stale={stale}
      empty={<p>Nothing</p>}
    />
  );
}

/** Whether `el` or an ancestor up to the document carries an opacity utility. */
function dimmed(el: HTMLElement): boolean {
  for (let at: HTMLElement | null = el; at; at = at.parentElement) {
    if (/(^|\s)opacity-\d/.test(at.className)) return true;
  }
  return false;
}

describe("stale_states", () => {
  test("data_table_stale_rows_keep_full_contrast", () => {
    const { rerender } = render(table(false));
    const items = screen.getByRole("table", { name: "Items" });
    expect(items).not.toHaveAttribute("aria-busy");
    expect(document.querySelector("[data-slot=stale-bar]")).toBeNull();

    rerender(table(true));
    expect(items).toHaveAttribute("aria-busy", "true");
    expect(document.querySelector("[data-slot=stale-bar]")).toHaveAttribute("aria-hidden", "true");
    expect(dimmed(screen.getByText("Alpha"))).toBe(false);
  });

  test("dashboard_panel_stale_figures_keep_full_contrast", () => {
    render(
      <Panel title="Traffic" busy stale>
        <p>1,048</p>
      </Panel>,
    );
    const panel = screen.getByRole("region", { name: "Traffic" });
    expect(panel).toHaveAttribute("aria-busy", "true");
    expect(panel.querySelector("[data-slot=stale-bar]")).not.toBeNull();
    expect(dimmed(screen.getByText("1,048"))).toBe(false);
  });

  test("media_grid_stale_tiles_keep_full_contrast", () => {
    const item = {
      id: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
      title: "Harbour",
      alt: "",
      mime: "image/png",
      bytes: 1,
      width: 10,
      height: 10,
      original_name: "harbour.png",
      url: "/_media/harbour.png",
      sanitized: false,
      status: "published",
      author_id: "u",
      author: { id: "u", login: "a", display_name: "A" },
      created_at: "2026-10-20T10:00:00Z",
      updated_at: "2026-10-20T10:00:00Z",
      version: 1,
      variants: [],
    } as MediaDto;
    render(
      <MediaGrid
        items={[item]}
        label="Library"
        loading={false}
        stale
        empty={null}
        selection={{ state: {}, onChange: () => {} }}
        onOpen={() => {}}
      />,
    );
    const grid = document.querySelector<HTMLElement>("[data-slot=media-grid]")!;
    expect(grid).toHaveAttribute("aria-busy", "true");
    expect(grid.querySelector("[data-slot=stale-bar]")).not.toBeNull();
    expect(dimmed(screen.getByRole("option", { name: "Harbour" }))).toBe(false);
  });

  test("no_loading_state_dims_its_content", () => {
    // Every screen keeps the previous answer at full colour while the next one loads
    // (TanStack Query's `isPlaceholderData`): no opacity utility tied to it.
    const files = import.meta.glob<string>("/src/**/*.tsx", {
      query: "?raw",
      import: "default",
      eager: true,
    });
    const tied =
      /isPlaceholderData[^;]{0,120}?\bopacity-\d|\bopacity-\d+[^;]{0,120}?isPlaceholderData/g;
    const found: string[] = [];
    for (const [path, source] of Object.entries(files)) {
      if (path.endsWith(".test.tsx")) continue;
      for (const m of source.matchAll(tied)) found.push(`${path}: ${m[0].replace(/\s+/g, " ")}`);
    }
    expect(found).toEqual([]);
  });
});
