// The palette's stale hits (P81): while a slow search runs, the previous query's rows stay
// at full colour (a dimmed row fell below 4.5:1); the hits are `aria-busy` and the header's
// search icon turns into a spinner instead.

import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilesIcon } from "lucide-react";
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import type { MeDto } from "@/auth/session";
import { DensityProvider } from "@/design/density";
import { ThemeProvider } from "@/design/theme";
import type { NavSection } from "@/shell/nav";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { CommandPalette } from "./palette";

beforeAll(() => {
  // cmdk scrolls the active row into view; jsdom has no layout.
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const ME = {
  id: "00000000-0000-0000-0000-00000000000a",
  login: "alice",
  capabilities: ["read_admin"],
} as unknown as MeDto;

const SECTIONS: NavSection[] = [
  {
    id: "content",
    label: "Content",
    items: [
      { id: "post", label: "Posts", to: "/content/post", icon: FilesIcon, contentType: "post" },
    ],
  },
];

const summary = (title: string) => ({
  id: "00000000-0000-0000-0000-000000000101",
  type: "post",
  title,
  slug: "harbour-notes",
  status: "publish",
  excerpt: "",
});

test("stale_rows_keep_full_contrast", async () => {
  let release = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  stubRoutes({
    "/api/v1/content/post": async (url) => {
      // The second query's answer waits until the stale state was checked.
      if (url.searchParams.get("q") === "harb") await held;
      return json({ items: [summary("Harbour notes")], next_cursor: null });
    },
  });
  renderWithRouter(
    <ThemeProvider>
      <DensityProvider>
        <CommandPalette
          open
          onOpenChange={vi.fn()}
          me={ME}
          sections={SECTIONS}
          onShortcuts={vi.fn()}
        />
      </DensityProvider>
    </ThemeProvider>,
  );
  const user = userEvent.setup();
  const input = await screen.findByRole("combobox");
  await user.type(input, "har");
  const list = screen.getByRole("listbox");
  await within(list).findByRole("option", { name: /Harbour notes/ });
  const content = list.querySelector<HTMLElement>("[data-slot=palette-content]")!;
  expect(content).not.toHaveAttribute("aria-busy");

  await user.type(input, "b");
  // The wait became noticeable (250 ms): the spinner and `aria-busy` say so.
  const spinner = await vi.waitFor(
    () => {
      const el = document.querySelector("[data-slot=palette-spinner]");
      if (!el) throw new Error("no spinner yet");
      return el;
    },
    { timeout: 2_000 },
  );
  expect(spinner).toHaveClass("animate-spin");
  expect(content).toHaveAttribute("aria-busy", "true");
  // Only the hits are busy: Go to, Create and Preferences are read as usual.
  expect(list).not.toHaveAttribute("aria-busy");
  // The previous query's row is still there, still selectable, at full colour.
  const stale = within(list).getByRole("option", { name: /Harbour notes/ });
  for (let el: HTMLElement | null = stale; el && el !== list; el = el.parentElement) {
    expect(el.className, "no dimming on the row or a parent").not.toMatch(/(^|\s)opacity-\d/);
  }
  expect(stale).not.toHaveAttribute("aria-disabled", "true");

  release();
  await vi.waitFor(() => expect(content).not.toHaveAttribute("aria-busy"));
  expect(document.querySelector("[data-slot=palette-spinner]")).toBeNull();
});
