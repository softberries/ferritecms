import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { ThemeDto } from "@/api/themes";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { monogram, nodesSettled, safeHomepage } from "./theme-model";
import { ThemesScreen } from "./themes-screen";

const NODE = "11111111-1111-4111-8111-111111111111";

function theme(over: Partial<ThemeDto> = {}): ThemeDto {
  return {
    name: "blog",
    version: "0.1.0",
    description: "A clean theme.",
    author: "FerriteCMS",
    license: "MIT",
    homepage: "https://example.com/blog",
    origin: "embedded",
    active: false,
    has_settings: true,
    hash: null,
    installed_at: null,
    menus: [],
    images: [],
    nodes: [],
    ...over,
  };
}

const served = (name: string, fallback = false, error: string | null = null) => ({
  node: NODE,
  node_name: "node-a",
  node_version: "0.1.0",
  name,
  hash: "abcd",
  origin: "artifact",
  fallback,
  error,
});

test("cards_show_manifest_facts_as_text", async () => {
  const hostile = theme({
    name: "evil",
    origin: "installed",
    description: '<img src=x onerror="alert(1)">',
    author: "<b>Mallory</b>",
    homepage: "javascript:alert(1)",
    hash: "f".repeat(64),
  });
  stubRoutes({
    "/api/v1/themes": () =>
      json({ items: [hostile, theme({ active: true, nodes: [served("blog")] })] }),
  });
  renderWithRouter(<ThemesScreen />);
  const cards = await screen.findAllByRole("listitem");
  // The active theme first.
  expect(cards[0]).toHaveTextContent("blog");
  const evil = cards.find((c) => c.textContent?.includes("evil"));
  expect(evil).toBeDefined();
  if (!evil) return;
  expect(within(evil).getByText('<img src=x onerror="alert(1)">')).toBeInTheDocument();
  expect(within(evil).getByText("By <b>Mallory</b>")).toBeInTheDocument();
  expect(evil.querySelector("img, b")).toBeNull();
  // Not http(s): text, never a link.
  expect(within(evil).getByText("javascript:alert(1)").closest("a")).toBeNull();
  expect(within(evil).getByText("Bundle ffffffffffff")).toBeInTheDocument();

  const blog = cards[0];
  if (!blog) return;
  const link = within(blog).getByRole("link", { name: /example\.com/ });
  expect(link).toHaveAttribute("href", "https://example.com/blog");
  expect(link).toHaveAttribute("rel", "noopener noreferrer nofollow");
  expect(within(blog).getByText("Active")).toBeInTheDocument();
  expect(within(blog).getByText("Built in")).toBeInTheDocument();
  expect(within(blog).getByText("Loaded")).toBeInTheDocument();
  // The active theme has no Activate; the other one has.
  expect(within(blog).queryByRole("button", { name: "Activate blog" })).toBeNull();
  expect(within(evil).getByRole("button", { name: "Activate evil" })).toBeInTheDocument();
});

test("fallback_node_shows_its_error", async () => {
  stubRoutes({
    "/api/v1/themes": () =>
      json({
        items: [theme({ active: true, nodes: [served("blog", true, "templates/x.html:1: boom")] })],
      }),
  });
  renderWithRouter(<ThemesScreen />);
  expect(await screen.findByText("Fallback")).toBeInTheDocument();
  expect(screen.getByText("templates/x.html:1: boom")).toBeInTheDocument();
});

test("activate_422_lists_template_problems", async () => {
  const seen = stubRoutes({
    "/api/v1/themes": () =>
      json({ items: [theme({ active: true }), theme({ name: "other", origin: "installed" })] }),
    "/api/v1/themes/other/activate": () =>
      new Response(
        JSON.stringify({
          type: "/api/v1/problems/validation",
          title: "Validation",
          status: 422,
          detail: "the theme does not load",
          errors: [{ detail: "templates/index.html:2: unexpected end of input" }],
        }),
        { status: 422, headers: { "Content-Type": "application/problem+json" } },
      ),
  });
  renderWithRouter(<ThemesScreen />);
  await userEvent.click(await screen.findByRole("button", { name: "Activate other" }));
  const dialog = await screen.findByRole("alertdialog", { name: "Activate other?" });
  expect(dialog).toHaveTextContent("blog stays installed");
  await userEvent.click(within(dialog).getByRole("button", { name: "Activate" }));
  expect(
    await within(dialog).findByText("templates/index.html:2: unexpected end of input"),
  ).toBeInTheDocument();
  expect(within(dialog).getByText("The theme does not load, so nothing changed:")).toBeVisible();
  expect(seen.filter((u) => u.pathname.endsWith("/activate"))).toHaveLength(1);
});

test("nodes_settled_needs_every_node_on_the_theme", () => {
  expect(nodesSettled(undefined, "blog")).toBe(false);
  expect(nodesSettled([theme({ active: true, nodes: [] })], "blog")).toBe(false);
  expect(nodesSettled([theme({ active: true, nodes: [served("old")] })], "blog")).toBe(false);
  expect(nodesSettled([theme({ active: true, nodes: [served("blog", true)] })], "blog")).toBe(
    false,
  );
  expect(nodesSettled([theme({ active: true, nodes: [served("blog")] })], "blog")).toBe(true);
  expect(nodesSettled([theme({ active: true, nodes: [served("blog")] })], "other")).toBe(false);
});

test("homepage_link_only_for_http", () => {
  expect(safeHomepage("https://a.example/x")).toBe("https://a.example/x");
  expect(safeHomepage("http://a.example")).toBe("http://a.example/");
  expect(safeHomepage("javascript:alert(1)")).toBeNull();
  expect(safeHomepage("data:text/html,x")).toBeNull();
  expect(safeHomepage("not a url")).toBeNull();
  expect(safeHomepage(null)).toBeNull();
});

test("monogram_tells_themes_apart", () => {
  expect(monogram("blog")).toBe("BL");
  expect(monogram("blog-min")).toBe("BM");
  expect(monogram("my_theme")).toBe("MT");
  expect(monogram("x")).toBe("X");
  expect(monogram("a-b-c")).toBe("AB");
});
