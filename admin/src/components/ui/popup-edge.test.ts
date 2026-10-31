// The edge of popup surfaces (dropdown menus, selects, popovers, hover cards): in dark
// mode they have no shadow, so the 1 px ring is what sets them off from the page. It is
// the `popover-edge` token (11 §3), not a 10 % tint of the foreground, which nearly
// vanished on the dark background (M4-4 Task 6).

import { describe, expect, test } from "vitest";
import dropdownMenu from "./dropdown-menu.tsx?raw";
import hoverCard from "./hover-card.tsx?raw";
import popover from "./popover.tsx?raw";
import select from "./select.tsx?raw";

const SURFACES = {
  "dropdown-menu.tsx": { source: dropdownMenu, count: 2 },
  "hover-card.tsx": { source: hoverCard, count: 1 },
  "popover.tsx": { source: popover, count: 1 },
  "select.tsx": { source: select, count: 1 },
};

describe("popup_edge", () => {
  test.each(Object.entries(SURFACES))("%s_draws_the_popover_edge_token", (_, { source, count }) => {
    expect(source).not.toContain("ring-foreground/10");
    expect(source.split("ring-1 ring-popover-edge").length - 1).toBe(count);
  });
});
