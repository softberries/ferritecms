// M6-1 Task 16: terms and menus per locale on the taxonomy and menu screens.

import { QueryClient } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { qk } from "@/api/keys";
import type { MenuDto } from "@/api/menus";
import type { SchemaMenuSummary } from "@/api/schema";
import type { TaxonomyDto, TermDto } from "@/api/taxonomies";
import { siteLocales } from "@/lib/locales";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { LocationLocales, TranslateMenu } from "./menus/menu-locales";
import { copyEntries } from "./menus/menu-locales-model";
import { TaxonomyScreen } from "./taxonomies/taxonomy-screen";
import { TermTranslations } from "./taxonomies/term-translations";

const LOCALES = [
  { tag: "en", label: "English", prefix: null },
  { tag: "de", label: "Deutsch", prefix: "de" },
];
const META = {
  api: "v1",
  site_url: "https://example.test",
  version: "0",
  default_locale: "en",
  locales: LOCALES,
};

function client(): QueryClient {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(qk.meta, META);
  return qc;
}

const CATEGORY: TaxonomyDto = {
  name: "category",
  label: "Categories",
  hierarchical: true,
  built_in: true,
  content_types: ["post"],
};

const term = (id: string, name: string, locale: string, group = id): TermDto => ({
  id,
  name,
  slug: name.toLowerCase(),
  description: "",
  taxonomy: "category",
  parent_id: null,
  locale,
  translation_group: group,
});

test("term_tree_filters_by_locale", async () => {
  const terms = [term("a", "News", "en"), term("b", "Nachrichten", "de", "a")];
  stubRoutes({
    "/api/v1/meta": () => json(META),
    "/api/v1/taxonomies/category/terms": () => json({ items: terms, next_cursor: null }),
    "/api/v1/types": () => json({ items: [], next_cursor: null }),
  });
  renderWithRouter(<TaxonomyScreen taxonomy={CATEGORY} canSettings={false} canTerms />, client());
  expect(await screen.findByText("News")).toBeInTheDocument();
  expect(screen.queryByText("Nachrichten")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: /Deutsch/ }));
  expect(await screen.findByText("Nachrichten")).toBeInTheDocument();
  expect(screen.queryByText("News")).toBeNull();
});

test("translate_term_posts_the_locale", async () => {
  const posted: unknown[] = [];
  stubRoutes({
    "/api/v1/meta": () => json(META),
    "/api/v1/terms/a/translations": async (_url, req) => {
      posted.push(await req.json());
      return json(term("c", "News", "de", "a"), 201);
    },
  });
  renderWithRouter(
    <TermTranslations
      term={term("a", "News", "en")}
      allTerms={[term("a", "News", "en")]}
      disabled={false}
    />,
    client(),
  );
  await userEvent.click(await screen.findByRole("button", { name: "Translate into Deutsch" }));
  await waitFor(() => expect(posted).toEqual([{ locale: "de" }]));
});

const summary = (name: string, locale: string, of: string | null = null): SchemaMenuSummary => ({
  name,
  label: name,
  locale,
  translation_of: of,
});

test("location_lists_menu_per_locale", async () => {
  stubRoutes({ "/api/v1/meta": () => json(META) });
  renderWithRouter(
    <ul>
      <LocationLocales
        name="primary"
        menus={[summary("primary", "en"), summary("primary-de", "de", "primary")]}
        locales={siteLocales(LOCALES, "en")}
      />
    </ul>,
    client(),
  );
  const de = (await screen.findByText("de")).closest("li");
  expect(de).not.toBeNull();
  expect(within(de as HTMLElement).getByRole("link", { name: "primary-de" })).toBeInTheDocument();
});

test("create_locale_menu_copies_items", async () => {
  const puts: { path: string; body: unknown }[] = [];
  stubRoutes({
    "/api/v1/meta": () => json(META),
    "/api/v1/menus/primary-de": async (url, req) => {
      const body = (await req.json()) as Record<string, unknown>;
      puts.push({ path: url.pathname, body });
      return json({ name: "primary-de", ...body });
    },
  });
  const menu: MenuDto = {
    name: "primary",
    label: "Main",
    locale: "en",
    translation_of: null,
    items: [
      { id: "r", kind: "url", url: "/", label: "Home", parent_id: null, position: 0 },
      { id: "c", kind: "url", url: "/a/", label: "A", parent_id: "r", position: 0 },
    ],
  };
  renderWithRouter(
    <TranslateMenu
      menu={menu}
      menus={[summary("primary", "en")]}
      locales={siteLocales(LOCALES, "en")}
      disabled={false}
    />,
    client(),
  );
  await userEvent.click(await screen.findByRole("button", { name: "Translate into Deutsch" }));
  await waitFor(() => expect(puts).toHaveLength(1));
  const body = puts[0]?.body as { locale: string; translation_of: string; items: MenuDto["items"] };
  expect(body.locale).toBe("de");
  expect(body.translation_of).toBe("primary");
  const [root, child] = body.items;
  expect(root?.id).not.toBe("r");
  expect(child?.parent_id).toBe(root?.id);
});

test("copy_entries_gives_fresh_ids", () => {
  const out = copyEntries([
    { id: "x", kind: "url", url: "/", label: "", parent_id: null, position: 0 },
  ]);
  expect(out[0]?.id).not.toBe("x");
});
