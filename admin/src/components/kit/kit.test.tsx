import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { CONTENT_STATUSES } from "@/api/content";
import type { MediaDto } from "@/api/media";
import { DataTable } from "./data-table";
import { dataTableColumns, type RowSelectionState } from "./data-table-features";
import { DropZone } from "./drop-zone";
import { MediaThumb } from "./media-thumb";
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

  it("drop_zone_takes_files_and_ignores_other_drags", async () => {
    const onFiles = vi.fn<(files: File[]) => void>();
    const onRejected = vi.fn<(files: File[]) => void>();
    const { container } = render(
      <DropZone onFiles={onFiles} onRejected={onRejected} accept="image/*,.pdf">
        <p>Library</p>
      </DropZone>,
    );
    const zone = container.querySelector("[data-slot=drop-zone]") as HTMLElement;
    const photo = new File(["x"], "photo.png", { type: "image/png" });
    const doc = new File(["x"], "notes.pdf", { type: "application/pdf" });
    const zip = new File(["x"], "site.zip", { type: "application/zip" });

    // A drag of text (no files) is left alone: no overlay, no drop target.
    const text = { types: ["text/plain"], files: [] };
    expect(fireEvent.dragOver(zone, { dataTransfer: text })).toBe(true);
    expect(screen.queryByText("Drop files to upload")).toBeNull();

    const files = { types: ["Files"], files: [photo, doc, zip] };
    fireEvent.dragEnter(zone, { dataTransfer: files });
    expect(screen.getByText("Drop files to upload")).toBeInTheDocument();
    // `preventDefault` on dragover is what makes the zone a drop target.
    expect(fireEvent.dragOver(zone, { dataTransfer: files })).toBe(false);
    fireEvent.drop(zone, { dataTransfer: files });
    expect(onFiles).toHaveBeenCalledWith([photo, doc]);
    expect(onRejected).toHaveBeenCalledWith([zip]);
    expect(screen.queryByText("Drop files to upload")).toBeNull();

    // The button opens the hidden picker; its choice goes the same way.
    const user = userEvent.setup();
    const input = container.querySelector("input[type=file]") as HTMLInputElement;
    expect(input).toHaveAttribute("multiple");
    expect(input).toHaveAttribute("accept", "image/*,.pdf");
    const clicked = vi.spyOn(input, "click");
    await user.click(screen.getByRole("button", { name: "Choose files" }));
    expect(clicked).toHaveBeenCalled();
    await user.upload(input, [photo]);
    expect(onFiles).toHaveBeenLastCalledWith([photo]);
  });

  it("media_thumb_picks_the_smallest_variant_that_fills_the_box", () => {
    const media = (over: Partial<MediaDto>): MediaDto => ({
      id: "0190f1c0-0000-7000-8000-0000000000aa",
      author_id: "0190f1c0-0000-7000-8000-0000000000bb",
      alt: "A lighthouse at dusk",
      title: "Lighthouse",
      original_name: "lighthouse.jpg",
      mime: "image/jpeg",
      bytes: 1000,
      sanitized: false,
      status: "published",
      url: "/media/lighthouse.jpg",
      width: 2000,
      height: 1500,
      variants: [
        { name: "large", url: "/media/l.jpg", width: 1024, height: 768, mime: "image/jpeg" },
        { name: "thumbnail", url: "/media/t.jpg", width: 150, height: 150, mime: "image/jpeg" },
        { name: "medium", url: "/media/m.jpg", width: 300, height: 225, mime: "image/jpeg" },
      ],
      version: 1,
      created_at: "2026-10-01T10:00:00Z",
      updated_at: "2026-10-01T10:00:00Z",
      ...over,
    });
    const src = (size: "sm" | "md" | "lg", m = media({})) => {
      const { container, unmount } = render(<MediaThumb media={m} size={size} />);
      const img = container.querySelector("img");
      const out = img?.getAttribute("src") ?? null;
      unmount();
      return out;
    };
    // sm 40 px, md 96 px: the 150 px thumbnail; lg 160 px: medium (its short side is 225).
    expect(src("sm")).toBe("/media/t.jpg");
    expect(src("md")).toBe("/media/t.jpg");
    expect(src("lg")).toBe("/media/m.jpg");
    // No variant big enough (or none made yet): the original.
    expect(src("lg", media({ variants: [] }))).toBe("/media/lighthouse.jpg");
    // SVG: the original, through <img> only.
    expect(src("md", media({ mime: "image/svg+xml", url: "/media/logo.svg", variants: [] }))).toBe(
      "/media/logo.svg",
    );

    render(<MediaThumb media={media({})} size="md" />);
    expect(screen.getByRole("img", { name: "A lighthouse at dusk" })).toBeInTheDocument();
    cleanup();

    // No alt: the image is decorative and the title is the visible text.
    const { container } = render(<MediaThumb media={media({ alt: "" })} size="md" />);
    expect(container.querySelector("img")).toHaveAttribute("alt", "");
    expect(screen.getByText("Lighthouse")).toBeVisible();
    cleanup();

    // Other kinds: no <img>, a type icon and the extension.
    const pdf = render(
      <MediaThumb
        media={media({
          mime: "application/pdf",
          original_name: "report.pdf",
          title: "Annual report",
          alt: "",
          variants: [],
        })}
        size="md"
      />,
    );
    expect(pdf.container.querySelector("img")).toBeNull();
    expect(screen.getByText("PDF")).toBeInTheDocument();
    expect(screen.getByText("Annual report")).toBeInTheDocument();
  });
});
