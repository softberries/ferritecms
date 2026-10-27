// The living styleguide (/admin/_styleguide, M2b P12): one review screen per section,
// axe in both schemes, the overlays opened, and density measured in the browser.

import { expect, test } from "./support/fixtures";
import { authFile } from "./support/server";
import { VIEWPORTS } from "./support/snap";

// Mirrors src/routes/_app/-styleguide/sections.ts (the nav is checked against it below).
const SECTIONS = [
  "colour",
  "type",
  "layout",
  "motion",
  "controls",
  "display",
  "navigation",
  "overlays",
  "patterns",
  "ordering",
  "data",
  "charts",
  "administration",
] as const;

test("styleguide_screens", async ({ as, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const page = await as("admin");

  await page.goto("/admin/_styleguide");
  const nav = page.getByRole("navigation", { name: "Styleguide sections" });
  // "All sections" plus one link per section, in order.
  await expect(nav.getByRole("link")).toHaveCount(SECTIONS.length + 1);
  for (const id of SECTIONS) {
    await expect(page.locator(`section[data-section=${id}]`)).toBeVisible();
  }

  for (const id of SECTIONS) {
    await page.goto(`/admin/_styleguide?section=${id}`);
    await expect(page.locator("section[data-section]")).toHaveCount(1);
    await expect(page.locator(`section[data-section=${id}]`)).toBeVisible();
    // No horizontal page scroll at phone width (11 §8); charts re-measure on a
    // ResizeObserver tick, so poll.
    await page.setViewportSize(VIEWPORTS.mobile);
    await expect
      .poll(
        () =>
          page.evaluate(
            () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
          ),
        { message: `${id}: horizontal overflow at 390 px` },
      )
      .toBe(0);
    await page.setViewportSize(VIEWPORTS.desktop);
    if (id === "data") {
      // Hover a row so its actions show.
      await page.getByRole("table", { name: "Posts" }).getByRole("row").nth(1).hover();
    }
    await snap(page, `styleguide-${id}`);
    await expectAccessible(page);
  }

  // Overlays, open.
  await page.goto("/admin/_styleguide?section=overlays");
  await page.getByRole("button", { name: "Actions" }).click();
  await expect(page.getByRole("menu")).toBeVisible();
  await snap(page, "styleguide-overlays-menu");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();

  await page.getByRole("button", { name: "Schedule…" }).click();
  const dialog = page.getByRole("dialog", { name: "Schedule post" });
  await expect(dialog).toBeVisible();
  await snap(page, "styleguide-overlays-dialog");
  await expectAccessible(page);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  // Focus returns to the trigger, which shows the standard ring.
  const trigger = page.getByRole("button", { name: "Schedule…" });
  await expect(trigger).toBeFocused();
  // (`transition-all` on buttons eases the ring in, so poll until it settles.)
  await expect
    .poll(() =>
      trigger.evaluate((el) => {
        const s = getComputedStyle(el);
        return `${s.outlineStyle} ${s.outlineWidth} ${s.outlineOffset}`;
      }),
    )
    .toBe("solid 2px 2px");
});

test("styleguide_compact_density", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/_styleguide?section=data");
  const table = page.getByRole("table", { name: "Posts" });
  const firstRow = table.getByRole("row").nth(1);
  const height = () => firstRow.evaluate((el) => el.getBoundingClientRect().height);
  await expect(page.locator("html")).toHaveAttribute("data-density", "comfortable");
  expect(await height()).toBe(44);

  await page.getByRole("radio", { name: "Compact" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-density", "compact");
  expect(await height()).toBe(34);
  // Controls follow --control-h (30 px compact).
  await page.goto("/admin/_styleguide?section=layout");
  const button = page.getByRole("button", { name: "Save", exact: true });
  expect(await button.evaluate((el) => el.getBoundingClientRect().height)).toBe(30);
});

test("styleguide_touch_targets", async ({ browser, backend, site, guard }) => {
  // On a touch screen controls and rows stay at least 44 px whatever the density (11 §4).
  const ctx = await browser.newContext({
    baseURL: site.url,
    storageState: authFile(backend, "admin"),
    viewport: { width: 390, height: 844 },
    hasTouch: true,
    isMobile: true,
  });
  await guard.watch(ctx);
  try {
    const page = await ctx.newPage();
    await page.goto("/admin/_styleguide?section=layout");
    expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
    await page.getByRole("radio", { name: "Compact" }).tap();
    await expect(page.locator("html")).toHaveAttribute("data-density", "compact");
    const button = page.getByRole("button", { name: "Save", exact: true });
    expect(await button.evaluate((el) => el.getBoundingClientRect().height)).toBe(44);
  } finally {
    await ctx.close();
  }
});

test("styleguide_table_frame_and_mixed_checkbox", async ({ as }) => {
  const page = await as("admin");
  await page.goto("/admin/_styleguide?section=data");
  // Every table scrolls inside its frame and never spills past its parent, at desktop
  // width too (review fix round 1); the header sticks to the frame.
  const frames = page.locator("[data-slot=data-table]");
  await expect(frames).toHaveCount(3);
  for (const frame of await frames.all()) {
    const box = await frame.evaluate((el) => {
      const s = getComputedStyle(el);
      const parent = el.parentElement?.getBoundingClientRect();
      return {
        overflowX: s.overflowX,
        spill: parent ? el.getBoundingClientRect().right - parent.right : 0,
        // The empty-state specimen has no header row (Task 13 fix round 1).
        sticky: (() => {
          const head = el.querySelector("thead");
          return head ? getComputedStyle(head).position : "sticky";
        })(),
      };
    });
    expect(box).toEqual({ overflowX: "auto", spill: 0, sticky: "sticky" });
  }

  // "Select all" with one row selected is mixed, and filled with primary in both schemes.
  const all = page.getByRole("table", { name: "Posts" }).getByRole("checkbox", {
    name: "Select all rows",
  });
  await expect(all).toHaveAttribute("data-state", "indeterminate");
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await expect(page.locator("html")).toHaveClass(scheme);
    await expect
      .poll(() =>
        all.evaluate((el) => {
          // A probe painted with the primary token, resolved like the checkbox fill.
          const probe = document.createElement("span");
          probe.style.backgroundColor = "var(--primary)";
          document.body.append(probe);
          const primary = getComputedStyle(probe).backgroundColor;
          probe.remove();
          return getComputedStyle(el).backgroundColor === primary;
        }),
      )
      .toBe(true);
  }
});

test("styleguide_ordering", async ({ as, snap, expectAccessible }) => {
  test.setTimeout(120_000);
  // The kit's sortable list and tree (menus and keyboard, as screens will drive them, and a
  // real pointer drag), the drop zone's overlay and its file filter.
  const page = await as("admin");
  await page.goto("/admin/_styleguide?section=ordering");
  const tree = page.getByRole("list", { name: "Main menu" });
  const treeStatus = page.locator("[data-slot=sortable-tree] ~ [data-slot=sortable-status]");
  const rowOf = (list: typeof tree, label: string) =>
    list
      .locator("[data-sortable-row]")
      .filter({ has: page.getByRole("button", { name: `Reorder ${label}`, exact: true }) });
  const order = (list: typeof tree) =>
    list
      .getByRole("button", { name: /^Reorder / })
      .evaluateAll((els) => els.map((el) => el.getAttribute("aria-label")?.slice(8)));

  // Text truncated with an ellipsis says itself in a `title` (11 §8).
  const untitled = await page
    .locator("main .truncate")
    .evaluateAll((els) => els.filter((el) => !el.closest("[title]")).map((el) => el.outerHTML));
  expect(untitled).toEqual([]);

  // Tree: Blog up among the top level, from its menu; focus stays on its menu button.
  // Each step waits for the menu to finish closing: reopening it during its exit
  // animation is not a user's pace.
  await page.getByRole("button", { name: "Actions for Blog" }).click();
  await expect(page.getByRole("menu")).toBeVisible();
  await snap(page, "styleguide-ordering-menu");
  await expectAccessible(page);
  await page.getByRole("menuitem", { name: "Move up" }).click();
  await expect(page.getByRole("menu")).toBeHidden();
  await expect
    .poll(() => order(tree))
    .toEqual(["Home", "Blog", "About", "Team", "History", "Contact"]);
  await expect(treeStatus).toHaveText("Blog moved to position 2 of 4.");
  await expect(page.getByRole("button", { name: "Actions for Blog" })).toBeFocused();

  // Contact: indent under About (its previous sibling), then out again.
  await page.getByRole("button", { name: "Actions for Contact" }).click();
  await page.getByRole("menuitem", { name: "Indent" }).click();
  await expect(page.getByRole("menu")).toBeHidden();
  await expect(rowOf(tree, "Contact")).toHaveAttribute("aria-level", "2");
  await expect(treeStatus).toHaveText("Contact is now under About, position 3 of 3.");
  await page.getByRole("button", { name: "Actions for Contact" }).click();
  // About's children sit at level 2 of the styleguide's maxDepth 2, so one more is allowed.
  await expect(page.getByRole("menuitem", { name: "Indent" })).toBeEnabled();
  await page.getByRole("menuitem", { name: "Outdent" }).click();
  await expect(page.getByRole("menu")).toBeHidden();
  await expect(rowOf(tree, "Contact")).toHaveAttribute("aria-level", "1");

  // Tree: a pointer drag of Contact 30 px to the right projects one level deeper (under
  // About, after History). The dashed guide sits exactly one indent (24 px) in: the
  // pointer's x moves it only through the projected depth, never on top of it.
  const frame = rowOf(tree, "Contact").locator(":scope > div");
  const before = await frame.boundingBox();
  const grip = await tree.getByRole("button", { name: "Reorder Contact" }).boundingBox();
  if (!before || !grip) throw new Error("no layout");
  const [gx, gy] = [grip.x + grip.width / 2, grip.y + grip.height / 2];
  await page.mouse.move(gx, gy);
  await page.mouse.down();
  await page.mouse.move(gx + 6, gy, { steps: 2 });
  await page.mouse.move(gx + 30, gy, { steps: 6 });
  const guide = tree.locator("[data-placeholder]");
  await expect(guide).toHaveCSS("margin-inline-start", "24px");
  await expect
    .poll(async () => Math.round((await guide.boundingBox())?.x ?? 0))
    .toBe(Math.round(before.x + 24));
  await page.mouse.up();
  await expect(rowOf(tree, "Contact")).toHaveAttribute("aria-level", "2");

  // List: the keyboard (Space, ArrowDown, Space) moves Title below Subtitle.
  const list = page.getByRole("list", { name: "Fields" });
  await list.getByRole("button", { name: "Reorder Title" }).focus();
  await page.keyboard.press("Space");
  await expect(page.locator("body")).toContainText("Picked up Title, position 1 of 5.");
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("body")).toContainText("Title is over position 2 of 5.");
  await page.keyboard.press("Space");
  await expect(page.locator("body")).toContainText("Title dropped at position 2 of 5.");
  await expect
    .poll(() => order(list))
    .toEqual(["Subtitle", "Title", "Hero image", "Body", "Reading time"]);

  // List: a pointer drag of Reading time to the top.
  const handle = list.getByRole("button", { name: "Reorder Reading time" });
  const from = await handle.boundingBox();
  const to = await list.getByRole("button", { name: "Reorder Subtitle" }).boundingBox();
  if (!from || !to) throw new Error("no layout");
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  // Past the 4 px activation distance, then in steps to above the first row.
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2 - 8, { steps: 2 });
  await page.mouse.move(to.x + to.width / 2, to.y + 2, { steps: 12 });
  await page.mouse.up();
  await expect
    .poll(() => order(list))
    .toEqual(["Reading time", "Subtitle", "Title", "Hero image", "Body"]);

  // Drop zone: a drag carrying files shows the overlay; the drop keeps what `accept` takes.
  const zone = page.locator("[data-slot=drop-zone]");
  const drag = (type: "dragenter" | "drop", names: string[]) =>
    zone.evaluate(
      (el, { type, names }) => {
        const dt = new DataTransfer();
        for (const name of names) {
          const mime = name.endsWith(".png") ? "image/png" : "application/x-msdownload";
          dt.items.add(new File(["x"], name, { type: mime }));
        }
        el.dispatchEvent(
          new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt }),
        );
      },
      { type, names },
    );
  await drag("dragenter", ["photo.png"]);
  await expect(page.getByText("Drop files to upload")).toBeVisible();
  await snap(page, "styleguide-ordering-drop");
  await expectAccessible(page);
  await drag("drop", ["photo.png", "setup.exe"]);
  await expect(page.getByText("Drop files to upload")).toBeHidden();
  await expect(page.getByText("1 file ready to upload")).toBeVisible();
  await expect(page.getByText("1 file is not a type this library takes")).toBeVisible();
});

test("styleguide_administration", async ({ as, snap, expectAccessible }) => {
  test.setTimeout(120_000);
  // The M2b-3 kit in its working states: a secret being replaced, a settings form with
  // edits pending (reset offered, a bad colour flagged), and the QR code black on white
  // in both schemes.
  const page = await as("admin");
  await page.goto("/admin/_styleguide?section=administration");
  const section = page.locator("section[data-section=administration]");

  const qr = section.getByRole("img", { name: "QR code for your authenticator app" });
  await expect(qr).toBeVisible();
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await expect(page.locator("html")).toHaveClass(scheme);
    const fills = await qr.evaluate((svg) => {
      const ground = svg.querySelector("rect:not([data-module])");
      return {
        ground: ground ? getComputedStyle(ground).fill : null,
        module: getComputedStyle(svg).fill,
      };
    });
    // Computed as the token wrote them: white ground, black modules, in both schemes.
    expect(fills).toEqual({ ground: "oklch(1 0 0)", module: "oklch(0 0 0)" });
  }
  await page.emulateMedia({ colorScheme: "light" });

  // The code input takes digits only.
  const code = section.getByLabel("Code from the app");
  await code.fill("");
  await code.pressSequentially("12a34");
  await expect(code).toHaveValue("1234");

  // Secret: Replace opens an empty password input.
  const form = section.locator("[data-slot=settings-form]");
  const apiKey = form.locator("[data-slot=setting][data-key=api_key]");
  await apiKey.getByRole("button", { name: "Replace" }).click();
  const secretInput = apiKey.getByLabel("API key");
  await expect(secretInput).toHaveAttribute("type", "password");
  await expect(secretInput).toHaveValue("");
  await secretInput.fill("not-a-real-key");

  // Settings: a changed tagline offers a reset; a bad colour is flagged and blocks saving.
  const tagline = form.getByLabel("Tagline");
  await tagline.fill("Iron, oxygen, time");
  const accent = form.getByLabel("Accent colour");
  await accent.fill("#12");
  await expect(form.getByText(/^Use a colour like #/)).toBeVisible();
  await expect(form.getByText("Stored value is not valid; the default is used")).toBeVisible();
  await expect(form.getByText("3 unsaved changes")).toBeVisible();
  // From the top: a full-page shot taken scrolled down draws the sticky header mid-page.
  await page.evaluate(() => window.scrollTo(0, 0));
  await snap(page, "styleguide-administration-editing");
  await expectAccessible(page);

  // A valid colour, then Save: only the changed keys go.
  await accent.fill(`#${"3b6ea5"}`);
  await form.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Would send: tagline, accent, api_key")).toBeVisible();
});
