// The live preview pane (design 15 §3.2–3.3, §11.1; P7 D16): a sandboxed frame on a
// signed link, reloaded on each refresh, "Save once to preview" for a new item, the device
// widths (stored), the status line, and the split divider's keyboard control.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { json, stubRoutes } from "@/test/render";
import { PreviewPane, SplitDivider } from "./preview-pane";
import { SPLIT_DEFAULT, storedDevice, storedSplit } from "./preview-prefs";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";

function wrap(ui: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{ui}</QueryClientProvider>;
}

function links() {
  stubRoutes({
    [`/api/v1/content/post/${ID}/preview-links`]: () =>
      json(
        {
          id: "l1",
          token: `ferp_${"a".repeat(43)}`,
          url: `http://localhost/_preview/link/ferp_${"a".repeat(43)}`,
          target_kind: "item",
          target_id: ID,
          created_by: "u",
          created_at: new Date().toISOString(),
          expires_at: new Date(Date.now() + 3_600_000).toISOString(),
        },
        201,
      ),
  });
}

beforeEach(() => {
  window.localStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const pane = (props: Partial<Parameters<typeof PreviewPane>[0]> = {}) => (
  <PreviewPane
    type="post"
    itemId={ID}
    title="Hello"
    refreshKey={0}
    updatedAt={null}
    onSave={() => {}}
    {...props}
  />
);

test("the_frame_is_sandboxed_without_same_origin_and_titled", async () => {
  links();
  render(wrap(pane()));
  const frame = await screen.findByTitle("Preview of Hello");
  await waitFor(() => expect(frame.getAttribute("src")).toMatch(/\?working=1$/));
  expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-popups allow-forms");
});

test("the_pane_reloads_on_each_refresh", async () => {
  links();
  const { rerender } = render(wrap(pane()));
  const frame = await screen.findByTitle("Preview of Hello");
  await waitFor(() => expect(frame.getAttribute("src")).toMatch(/\?working=1$/));
  rerender(wrap(pane({ refreshKey: 3, updatedAt: new Date(2026, 10, 3, 12, 4, 31) })));
  await waitFor(() => expect(frame.getAttribute("src")).toMatch(/\?working=1&r=3$/));
  const status = screen.getByRole("status");
  expect(status).toHaveAttribute("aria-live", "polite");
  expect(status).toHaveTextContent(/^Updated 12:04:31$/);
});

test("the_status_says_the_saved_version_without_a_working_copy", () => {
  links();
  render(wrap(pane()));
  expect(screen.getByRole("status")).toHaveTextContent("Showing the saved version");
});

test("a_new_item_asks_for_a_save_first", async () => {
  const seen = stubRoutes({});
  const onSave = vi.fn();
  render(wrap(pane({ itemId: null, onSave })));
  expect(screen.getByText("Save once to preview")).toBeInTheDocument();
  expect(screen.queryByTitle(/Preview of/)).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(onSave).toHaveBeenCalledTimes(1);
  expect(seen).toEqual([]);
});

test("device_widths_are_offered_and_stored", async () => {
  links();
  render(wrap(pane()));
  const frame = await screen.findByTitle("Preview of Hello");
  const box = frame.parentElement as HTMLElement;
  expect(box.style.width).toBe("100%");
  await userEvent.click(screen.getByRole("radio", { name: "Tablet" }));
  expect(box.style.width).toBe("768px");
  expect(storedDevice()).toBe("tablet");
  await userEvent.click(screen.getByRole("radio", { name: "Phone" }));
  expect(box.style.width).toBe("390px");
  expect(storedDevice()).toBe("phone");
});

test("the_split_divider_moves_by_keyboard_within_its_limits", () => {
  const onChange = vi.fn();
  const { rerender } = render(<SplitDivider value={50} onChange={onChange} />);
  const sep = screen.getByRole("separator", { name: "Resize the preview" });
  expect(sep).toHaveAttribute("aria-valuenow", "50");
  expect(sep).toHaveAttribute("aria-valuemin", "35");
  expect(sep).toHaveAttribute("aria-valuemax", "65");
  expect(sep).toHaveAttribute("tabindex", "0");
  fireEvent.keyDown(sep, { key: "ArrowLeft" });
  expect(onChange).toHaveBeenLastCalledWith(45);
  fireEvent.keyDown(sep, { key: "ArrowRight" });
  expect(onChange).toHaveBeenLastCalledWith(55);
  fireEvent.keyDown(sep, { key: "Home" });
  expect(onChange).toHaveBeenLastCalledWith(35);
  fireEvent.keyDown(sep, { key: "End" });
  expect(onChange).toHaveBeenLastCalledWith(65);
  rerender(<SplitDivider value={65} onChange={onChange} />);
  fireEvent.keyDown(sep, { key: "ArrowRight" });
  expect(onChange).toHaveBeenLastCalledWith(65);
});

test("the_split_is_stored_and_clamped", async () => {
  const { storeSplit } = await import("./preview-prefs");
  expect(storedSplit()).toBe(SPLIT_DEFAULT);
  storeSplit(60);
  expect(storedSplit()).toBe(60);
  window.localStorage.setItem("ferrite.admin.previewSplit", "90");
  expect(storedSplit()).toBe(65);
  window.localStorage.setItem("ferrite.admin.previewSplit", "nope");
  expect(storedSplit()).toBe(SPLIT_DEFAULT);
});
