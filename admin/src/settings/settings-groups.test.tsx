import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { QueryClient, useQuery } from "@tanstack/react-query";
import { qk } from "@/api/keys";
import { type SettingsDto, settingsQuery } from "@/api/settings";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { invalid } from "@/users/fixtures";
import { GeneralGroup } from "./general";
import { PermalinksGroup } from "./permalinks";

const STORED: SettingsDto = {
  "site.title": "Old title",
  "site.description": null,
  "site.locale": "en",
  "reading.per_page": null,
  "reading.front_page": null,
  "reading.discourage_indexing": null,
  "permalink.post": null,
  "permalink.page": null,
  "comments.enabled": null,
  "comments.moderation": null,
  "comments.per_page": null,
};

/** `/api/v1/settings`: `GET` answers `theirs`; `PATCH` bodies are collected. */
function settingsApi(theirs: SettingsDto, onPatch?: (body: SettingsDto) => Response) {
  const patches: SettingsDto[] = [];
  const seen = stubRoutes({
    "/api/v1/meta": () => json({ api: "v1", site_url: "https://example.test", version: "0" }),
    "/api/v1/settings": async (_url, req) => {
      if (req.method === "PATCH") {
        const body = (await req.json()) as SettingsDto;
        patches.push(body);
        return onPatch?.(body) ?? json({ ...theirs, ...body });
      }
      return json(theirs);
    },
  });
  return { patches, seen };
}

test("general_saves_only_its_changed_keys", async () => {
  const { patches } = settingsApi(STORED);
  renderWithRouter(<GeneralGroup settings={STORED} />);
  const title = await screen.findByRole("textbox", { name: "Site title" });
  await userEvent.clear(title);
  await userEvent.type(title, "  New title ");
  expect(screen.getByText("1 unsaved change")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(patches).toEqual([{ "site.title": "New title" }]));
  // Saved: nothing is pending any more.
  await waitFor(() => expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled());
});

test("empty_title_resets_to_the_config_value", async () => {
  const { patches } = settingsApi(STORED);
  renderWithRouter(<GeneralGroup settings={STORED} />);
  await userEvent.clear(await screen.findByRole("textbox", { name: "Site title" }));
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(patches).toEqual([{ "site.title": null }]));
});

test("limits_are_checked_before_the_save", async () => {
  const { patches } = settingsApi(STORED);
  renderWithRouter(<GeneralGroup settings={STORED} />);
  const locale = await screen.findByRole("textbox", { name: "Language" });
  await userEvent.clear(locale);
  await userEvent.type(locale, "pt BR");
  expect(
    screen.getByText("Use 1 to 35 letters, digits, “-” or “_”, such as en or pt-BR."),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(patches).toEqual([]);
  await waitFor(() => expect(locale).toHaveFocus());
  expect(locale).toHaveAttribute("aria-invalid", "true");
  const title = screen.getByRole("textbox", { name: "Site title" });
  await userEvent.clear(title);
  await userEvent.click(title);
  await userEvent.paste("x".repeat(201));
  expect(screen.getByText("At most 200 characters.")).toBeInTheDocument();
});

test("server_422_shows_at_its_field", async () => {
  const { patches } = settingsApi(STORED, () =>
    invalid("site.locale must be 1 to 35 ASCII letters", "site.locale"),
  );
  renderWithRouter(<GeneralGroup settings={STORED} />);
  const locale = await screen.findByRole("textbox", { name: "Language" });
  await userEvent.clear(locale);
  await userEvent.type(locale, "de");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(await screen.findByText("site.locale must be 1 to 35 ASCII letters")).toBeInTheDocument();
  expect(patches).toHaveLength(1);
  expect(locale).toHaveAttribute("aria-invalid", "true");
});

test("changed_by_someone_else_asks_before_overwriting", async () => {
  // Someone else changed the title since the form opened.
  const { patches } = settingsApi({ ...STORED, "site.title": "Their title" });
  renderWithRouter(<GeneralGroup settings={STORED} />);
  const tagline = await screen.findByRole("textbox", { name: "Tagline" });
  await userEvent.type(tagline, "Mine");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(await screen.findByRole("alertdialog", { name: "Changed by someone else" })).toBeVisible();
  expect(patches).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: "Overwrite" }));
  await waitFor(() => expect(patches).toEqual([{ "site.description": "Mine" }]));
});

test("another_groups_change_is_not_a_conflict", async () => {
  // A change to a key outside the group does not ask.
  const { patches } = settingsApi({ ...STORED, "comments.per_page": 20 });
  renderWithRouter(<GeneralGroup settings={STORED} />);
  await userEvent.type(await screen.findByRole("textbox", { name: "Tagline" }), "Mine");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(patches).toEqual([{ "site.description": "Mine" }]));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

test("permalink_change_confirms_first", async () => {
  const { patches } = settingsApi(STORED);
  renderWithRouter(<PermalinksGroup settings={STORED} />);
  await userEvent.click(await screen.findByRole("radio", { name: /Month and name/ }));
  // The live preview follows the structure.
  const year = String(new Date().getUTCFullYear());
  expect(
    await screen.findByText(new RegExp(`example\\.test/${year}/\\d{2}/sample-post/`)),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Change the permalinks?" });
  expect(confirm).toHaveTextContent("Redirects from the old URLs are created in the background");
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(patches).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await userEvent.click(await screen.findByRole("button", { name: "Change permalinks" }));
  await waitFor(() => expect(patches).toEqual([{ "permalink.post": "/%year%/%month%/%slug%/" }]));
});

test("page_structure_without_parent_path_is_refused", async () => {
  const { patches } = settingsApi(STORED);
  renderWithRouter(<PermalinksGroup settings={STORED} />);
  const page = await screen.findByRole("textbox", { name: "Page structure" });
  await userEvent.clear(page);
  await userEvent.type(page, "/%slug%/");
  expect(
    screen.getByText("Pages need %parent_path%, or sub-pages would share addresses."),
  ).toBeInTheDocument();
  expect(screen.getByText("Fix the structure to see a preview.")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(patches).toEqual([]);
  // A chip adds its token as the last segment.
  await userEvent.clear(page);
  await userEvent.type(page, "/docs/");
  await userEvent.click(screen.getAllByRole("button", { name: "Add %parent_path%" }).at(-1)!);
  await userEvent.click(screen.getAllByRole("button", { name: "Add %slug%" }).at(-1)!);
  expect(page).toHaveValue("/docs/%parent_path%/%slug%/");
  expect(screen.getByText(/example\.test\/docs\/about\/team\//)).toBeInTheDocument();
});

test("permalink_confirm_asks_before_saving_starts", async () => {
  settingsApi(STORED);
  renderWithRouter(<PermalinksGroup settings={STORED} />);
  await userEvent.click(await screen.findByRole("radio", { name: /Month and name/ }));
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await screen.findByRole("alertdialog", { name: "Change the permalinks?" });
  // Only asking: the form is not saving and its fields stay enabled behind the dialog.
  expect(screen.queryByRole("button", { name: "Saving…", hidden: true })).toBeNull();
  expect(screen.getByRole("textbox", { name: "Page structure", hidden: true })).toBeEnabled();
});

/** The group as the screen renders it: its settings from the query cache, seeded with `STORED`. */
function renderLive(Group: typeof PermalinksGroup) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(qk.settings, STORED);
  function Live() {
    const settings = useQuery(settingsQuery);
    return settings.data ? <Group settings={settings.data} /> : null;
  }
  renderWithRouter(<Live />, client);
}

test("preset_follows_the_structure_after_reload", async () => {
  // Someone else chose "Category and name" meanwhile.
  settingsApi({ ...STORED, "permalink.post": "/%category%/%slug%/" });
  renderLive(PermalinksGroup);
  await userEvent.click(await screen.findByRole("radio", { name: /Custom/ }));
  const custom = screen.getByRole("textbox", { name: "Custom structure" });
  await userEvent.clear(custom);
  await userEvent.type(custom, "/blog/%slug%/");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await userEvent.click(await screen.findByRole("button", { name: "Change permalinks" }));
  await userEvent.click(await screen.findByRole("button", { name: "Reload" }));
  await waitFor(() =>
    expect(screen.getByRole("radio", { name: /Category and name/ })).toBeChecked(),
  );
  expect(screen.queryByRole("textbox", { name: "Custom structure" })).toBeNull();
});

test("preset_follows_the_structure_after_save", async () => {
  const { patches } = settingsApi(STORED);
  renderLive(PermalinksGroup);
  await userEvent.click(await screen.findByRole("radio", { name: /Custom/ }));
  const custom = screen.getByRole("textbox", { name: "Custom structure" });
  await userEvent.clear(custom);
  await userEvent.type(custom, "/%year%/%month%/%slug%/");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await userEvent.click(await screen.findByRole("button", { name: "Change permalinks" }));
  await waitFor(() => expect(patches).toHaveLength(1));
  await waitFor(() => expect(screen.getByRole("radio", { name: /Month and name/ })).toBeChecked());
});
