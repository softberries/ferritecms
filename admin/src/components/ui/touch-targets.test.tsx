// Touch targets (11 §4, §8): on a coarse pointer every button is at least 44 px, the
// small icon sizes included. jsdom has no layout, so the class is what is checked; the
// E2E `styleguide_touch_targets` measures the boxes in a touch browser.

import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { Button } from "./button";
import { InputGroup, InputGroupAddon, InputGroupButton } from "./input-group";

describe("touch_targets", () => {
  test.each(["icon-xs", "icon-sm"] as const)("button_%s_is_44px_under_a_coarse_pointer", (size) => {
    render(
      <Button size={size} aria-label="Close">
        x
      </Button>,
    );
    expect(screen.getByRole("button", { name: "Close" })).toHaveClass("pointer-coarse:size-11");
  });

  // Inside a field (itself 44 px tall on touch screens) the button keeps its look; a
  // pseudo-element widens what a finger hits to 44 px: 24 + 2 × 10 and 32 + 2 × 6.
  test.each([
    ["icon-xs", "pointer-coarse:after:-inset-2.5"],
    ["icon-sm", "pointer-coarse:after:-inset-1.5"],
  ] as const)("input_group_button_%s_hits_44px_under_a_coarse_pointer", (size, inset) => {
    render(
      <InputGroup>
        <InputGroupAddon align="inline-end">
          <InputGroupButton size={size} aria-label="Clear">
            x
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>,
    );
    expect(screen.getByRole("button", { name: "Clear" })).toHaveClass(
      "relative",
      "pointer-coarse:after:absolute",
      inset,
    );
  });
});
