import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import type { EditorDocument, SeoDraft } from "../editor-state";
import { SeoSection } from "./seo-section";

/** The members of the document `SeoSection` reads, around a `setMeta` spy. */
function docWith(seo: SeoDraft, fieldErrors: Record<string, string> = {}) {
  const setMeta = vi.fn();
  const clearFieldError = vi.fn();
  const doc = {
    title: "Harbour notes",
    item: { permalink: "/harbour-notes/" },
    meta: { excerpt: "", seo },
    fieldErrors,
    setMeta,
    clearFieldError,
  } as unknown as EditorDocument;
  return { doc, setMeta, clearFieldError };
}

const EMPTY: SeoDraft = { title: "", description: "", canonical: "", robots: [] };

function renderSeo(doc: EditorDocument) {
  return render(
    <SeoSection doc={doc} siteUrl="https://example.com" siteName="Example" disabled={false} />,
  );
}

describe("seo_section", () => {
  test("canonical error from seo.canonical shows under the field", () => {
    const { doc } = docWith(
      { ...EMPTY, canonical: "example.com/a" },
      { "seo.canonical": "seo.canonical must be an absolute http(s) URL" },
    );
    renderSeo(doc);
    const input = screen.getByRole("textbox", { name: "Canonical URL" });
    expect(input).toHaveAttribute("type", "url");
    expect(input).toHaveAttribute("aria-invalid", "true");
    // The server's message, as a sentence without the path, described by the input.
    expect(input).toHaveAccessibleDescription(
      expect.stringContaining("Must be an absolute http(s) URL"),
    );
    // The item's own address is the default the field shows.
    expect(input).toHaveAttribute("placeholder", "https://example.com/harbour-notes/");
  });

  test("an edited canonical clears its server error", () => {
    const { doc, setMeta, clearFieldError } = docWith(EMPTY, {
      "seo.canonical": "seo.canonical must be an absolute http(s) URL",
    });
    renderSeo(doc);
    fireEvent.change(screen.getByRole("textbox", { name: "Canonical URL" }), {
      target: { value: "https://elsewhere.example/" },
    });
    expect(clearFieldError).toHaveBeenCalledWith("seo.canonical");
    expect(setMeta).toHaveBeenCalledWith({
      seo: { ...EMPTY, canonical: "https://elsewhere.example/" },
    });
  });

  test("a canonical that is not a full address is named once the field is left", () => {
    const { doc } = docWith({ ...EMPTY, canonical: "elsewhere.example/a" });
    renderSeo(doc);
    const input = screen.getByRole("textbox", { name: "Canonical URL" });
    expect(input).not.toHaveAttribute("aria-invalid");
    fireEvent.blur(input);
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription(expect.stringContaining("https://"));
  });

  test("robots toggles are a labelled group and keep the directives sorted", () => {
    const { doc, setMeta } = docWith({ ...EMPTY, robots: ["nofollow"] });
    renderSeo(doc);
    const group = screen.getByRole("group", { name: "Search engine directives" });
    const switches = within(group).getAllByRole("switch");
    expect(switches).toHaveLength(5);
    expect(within(group).getByRole("switch", { name: "Do not follow links" })).toBeChecked();
    fireEvent.click(within(group).getByRole("switch", { name: "Hide from search results" }));
    expect(setMeta).toHaveBeenLastCalledWith({
      seo: { ...EMPTY, robots: ["nofollow", "noindex"] },
    });
    fireEvent.click(within(group).getByRole("switch", { name: "Do not follow links" }));
    expect(setMeta).toHaveBeenLastCalledWith({ seo: { ...EMPTY, robots: [] } });
  });

  test("reset clears all four", () => {
    const { doc, setMeta } = docWith({
      title: "Search title",
      description: "A description.",
      canonical: "https://elsewhere.example/orig/",
      robots: ["noindex", "nosnippet"],
    });
    renderSeo(doc);
    fireEvent.click(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(setMeta).toHaveBeenCalledWith({ seo: EMPTY });
  });

  test("reset shows while only a directive is set", () => {
    const { doc } = docWith({ ...EMPTY, robots: ["noarchive"] });
    renderSeo(doc);
    expect(screen.getByRole("button", { name: "Reset to defaults" })).toBeVisible();
  });

  test("noindex is noted at the search preview", () => {
    const { doc } = docWith({ ...EMPTY, robots: ["noindex"] });
    renderSeo(doc);
    expect(screen.getByText("Search engines are asked not to list this page.")).toBeVisible();
  });

  test("the preview shows the canonical address once it is a full one", () => {
    const { doc } = docWith({ ...EMPTY, canonical: "https://elsewhere.example/orig/" });
    const { container } = renderSeo(doc);
    expect(container.querySelector("[data-slot=search-preview]")).toHaveTextContent(
      "elsewhere.example › orig",
    );
  });
});
