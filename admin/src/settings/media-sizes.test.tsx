import { screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { MediaSizesGroup } from "./media-sizes";

const theme = (images: unknown[]) => ({
  name: "studio",
  version: "1.0.0",
  active: true,
  origin: "installed",
  has_settings: false,
  menus: [],
  nodes: [],
  images,
});

test("sizes_without_height_and_skipped_sizes", async () => {
  stubRoutes({
    "/api/v1/themes": () =>
      json({
        items: [
          theme([
            { name: "wide", width: 1200, height: null, crop: false },
            { name: "square", width: 300, height: null, crop: true },
            { name: "box", width: 800, height: 600, crop: false },
            { name: "flat", width: 0, height: null, crop: false },
            { name: "medium", width: 500, height: null, crop: false },
          ]),
        ],
      }),
  });
  renderWithRouter(<MediaSizesGroup canThemes />);
  expect(await screen.findByText("Set by the theme studio")).toBeInTheDocument();
  const row = (name: string) => document.querySelector<HTMLElement>(`tr[data-size="${name}"]`)!;
  // No height: scaled to the width; with crop, a square.
  expect(within(row("wide")).getByText("1200 × auto")).toBeInTheDocument();
  expect(within(row("wide")).queryByText("Not made")).toBeNull();
  expect(within(row("square")).getByText("300 × 300")).toBeInTheDocument();
  expect(within(row("square")).getByText("Cropped")).toBeInTheDocument();
  // What the processor skips, and why.
  expect(within(row("box")).getByText("Not made")).toBeInTheDocument();
  expect(within(row("box")).getByText("A size with a height must be cropped.")).toBeInTheDocument();
  expect(within(row("flat")).getByText("A size cannot be 0 pixels.")).toBeInTheDocument();
  expect(
    within(row("medium")).getByText("FerriteCMS already makes a size of this name."),
  ).toBeInTheDocument();
});
