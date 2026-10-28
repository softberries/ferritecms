import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, expect, test, vi } from "vitest";
import type { SettingDefDto } from "@/api/themes";
import { renderWithRouter } from "@/test/render";
import { SettingsForm, type SettingsFormProps } from "./settings-form";

beforeAll(() => {
  // Radix Select measures and captures the pointer; jsdom has neither.
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

// Colour values built from parts: source files hold no colour literal (11 §3).
const hex = (digits: string) => `#${digits}`;

const def = (
  key: string,
  type: string,
  label: string,
  dflt: unknown = null,
  options: string[] = [],
): SettingDefDto => ({ key, type, label, default: dflt, options });

const SCHEMA: SettingDefDto[] = [
  def("title", "text", "Title", "Hello"),
  def("intro", "textarea", "Intro", ""),
  def("dark", "bool", "Dark header", false),
  def("columns", "number", "Columns", 2),
  def("layout", "select", "Layout", "list", ["list", "grid"]),
  def("accent", "color", "Accent", hex("aa3300")),
  def("logo", "media", "Logo"),
  def("api_key", "secret", "API key"),
];

// As `GET …/settings` answers: every non-secret value as read (defaults filled in).
const VALUES: Record<string, unknown> = {
  title: "Hello",
  intro: "",
  dark: false,
  columns: 2,
  layout: "list",
  accent: hex("aa3300"),
  logo: null,
};

async function setup(over: Partial<SettingsFormProps> = {}) {
  const onSubmit = vi.fn<SettingsFormProps["onSubmit"]>(() => Promise.resolve());
  // Unchanged on the server unless the test says otherwise.
  const reload = vi.fn(() => Promise.resolve({ ...(over.values ?? VALUES) }));
  renderWithRouter(
    <SettingsForm
      schema={SCHEMA}
      values={VALUES}
      secrets={{ api_key: true }}
      reload={reload}
      onSubmit={onSubmit}
      staleKey="Theme blog"
      {...over}
    />,
  );
  // The router renders on its next tick.
  await screen.findByRole("button", { name: "Save changes" });
  return { onSubmit, reload };
}

const field = (key: string) => {
  const el = document.querySelector<HTMLElement>(`[data-slot=setting][data-key="${key}"]`);
  if (!el) throw new Error(`no field ${key}`);
  return within(el);
};
const save = () => userEvent.click(screen.getByRole("button", { name: "Save changes" }));

test("each_kind_renders_and_round_trips", async () => {
  const user = userEvent.setup();
  const { onSubmit } = await setup();
  expect(screen.getByLabelText("Title")).toHaveValue("Hello");
  expect(screen.getByLabelText("Intro").tagName).toBe("TEXTAREA");
  expect(screen.getByRole("switch", { name: "Dark header" })).not.toBeChecked();
  expect(screen.getByLabelText("Columns")).toHaveValue("2");
  expect(screen.getByRole("combobox", { name: "Layout" })).toHaveTextContent("list");
  expect(screen.getByLabelText("Accent")).toHaveValue(hex("aa3300"));
  expect(field("logo").getByRole("button")).toHaveTextContent("Choose a file");
  expect(field("api_key").getByText("Set")).toBeInTheDocument();
  // Nothing changed yet: nothing to save.
  expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();

  await user.clear(screen.getByLabelText("Title"));
  await user.type(screen.getByLabelText("Title"), "My site");
  await user.type(screen.getByLabelText("Intro"), "Line one");
  await user.click(screen.getByRole("switch", { name: "Dark header" }));
  await user.clear(screen.getByLabelText("Columns"));
  await user.type(screen.getByLabelText("Columns"), "3");
  await user.click(screen.getByRole("combobox", { name: "Layout" }));
  await user.click(await screen.findByRole("option", { name: "grid" }));
  await user.clear(screen.getByLabelText("Accent"));
  await user.type(screen.getByLabelText("Accent"), hex("123ABC"));
  await user.click(field("api_key").getByRole("button", { name: "Replace" }));
  await user.type(screen.getByLabelText("API key"), "k-123");
  await save();

  await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
  expect(onSubmit.mock.calls[0]?.[0]).toEqual({
    title: "My site",
    intro: "Line one",
    dark: true,
    columns: 3,
    layout: "grid",
    accent: hex("123abc"),
    api_key: "k-123",
  });
});

test("only_changed_keys_are_sent", async () => {
  const { onSubmit } = await setup();
  await userEvent.click(screen.getByRole("switch", { name: "Dark header" }));
  // Changed and changed back: not sent.
  await userEvent.type(screen.getByLabelText("Title"), "!");
  await userEvent.type(screen.getByLabelText("Title"), "{Backspace}");
  await save();
  await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
  // The untouched secret is absent (kept), as is every other key.
  expect(onSubmit.mock.calls[0]?.[0]).toEqual({ dark: true });
});

test("color_kind_takes_rrggbb_only", async () => {
  const { onSubmit } = await setup();
  const input = screen.getByLabelText("Accent");
  await userEvent.clear(input);
  await userEvent.type(input, "#12");
  expect(field("accent").getByText(/^Use a colour like #/)).toBeInTheDocument();
  expect(input).toHaveAttribute("aria-invalid", "true");
  await save();
  expect(onSubmit).not.toHaveBeenCalled();
  // The swatch picker writes the hex field too.
  fireEvent.input(field("accent").getByLabelText("Pick a colour"), {
    target: { value: hex("00ff00") },
  });
  expect(input).toHaveValue(hex("00ff00"));
  await save();
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ accent: hex("00ff00") }));
});

test("reset_to_default_sends_null_and_invalid_is_flagged", async () => {
  const { onSubmit } = await setup({
    values: { ...VALUES, title: "Custom", columns: 2 },
    invalid: ["columns"],
  });
  // The stored value of `columns` does not fit its definition: it reads as the default.
  expect(
    field("columns").getByText("Stored value is not valid; the default is used"),
  ).toBeInTheDocument();
  expect(field("title").getByText("Default: Hello")).toBeInTheDocument();
  await userEvent.click(field("title").getByRole("button", { name: "Reset to default" }));
  expect(screen.getByLabelText("Title")).toHaveValue("Hello");
  await userEvent.click(field("columns").getByRole("button", { name: "Reset to default" }));
  // A field at its default (and valid) offers no reset.
  expect(field("layout").queryByRole("button", { name: "Reset to default" })).toBeNull();
  await save();
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ title: null, columns: null }));
});

test("secret_remove_sends_null", async () => {
  const { onSubmit } = await setup();
  await userEvent.click(field("api_key").getByRole("button", { name: "Remove" }));
  await save();
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ api_key: null }));
});

test("a_stale_value_asks", async () => {
  const theirs = { ...VALUES, title: "Theirs" };
  const reload = vi.fn(() => Promise.resolve(theirs));
  const { onSubmit } = await setup({ reload });

  await userEvent.click(screen.getByRole("switch", { name: "Dark header" }));
  await save();
  const dialog = await screen.findByRole("alertdialog");
  expect(within(dialog).getByText("Changed by someone else")).toBeInTheDocument();
  expect(within(dialog).getByText(/Theme blog/)).toBeInTheDocument();
  // Reload: no write; the form shows their values and drops the edit.
  await userEvent.click(within(dialog).getByRole("button", { name: "Reload" }));
  expect(onSubmit).not.toHaveBeenCalled();
  expect(screen.getByLabelText("Title")).toHaveValue("Theirs");
  expect(screen.getByRole("switch", { name: "Dark header" })).not.toBeChecked();

  // The next save compares with what was reloaded: no question, only the change goes.
  await userEvent.click(screen.getByRole("switch", { name: "Dark header" }));
  await save();
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ dark: true }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

test("a_stale_value_can_be_overwritten", async () => {
  const reload = vi.fn(() => Promise.resolve({ ...VALUES, columns: 5 }));
  const { onSubmit } = await setup({ reload });
  await userEvent.click(screen.getByRole("switch", { name: "Dark header" }));
  await save();
  await userEvent.click(await screen.findByRole("button", { name: "Overwrite" }));
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ dark: true }));
});

test("server_field_errors_show_at_their_field", async () => {
  const { ApiError } = await import("@/api/problem");
  const onSubmit = vi.fn(() =>
    Promise.reject(
      new ApiError({
        type: "/api/v1/problems/validation",
        title: "Validation failed",
        status: 422,
        detail: "One setting is not valid.",
        errors: [{ field: "title", detail: "At most 1000 characters." }],
      }),
    ),
  );
  await setup({ onSubmit });
  await userEvent.type(screen.getByLabelText("Title"), "x");
  await save();
  expect(await field("title").findByText("At most 1000 characters.")).toBeInTheDocument();
  expect(screen.getByLabelText("Title")).toHaveAttribute("aria-invalid", "true");
});

test("empty_values_and_short_colours_read_as_defaults", async () => {
  const schema = [
    // No default: the server reads it as its kind's empty value ("").
    def("note", "text", "Note"),
    def("tint", "color", "Tint", hex("abc")),
    def("edge", "color", "Edge", hex("abc")),
  ];
  const values = { note: "", tint: hex("AABBCC"), edge: hex("112233") };
  const { onSubmit } = await setup({ schema, values });
  // At rest and equal to what a reset gives: no Reset.
  expect(field("note").queryByRole("button", { name: "Reset to default" })).toBeNull();
  expect(field("tint").queryByRole("button", { name: "Reset to default" })).toBeNull();
  // A real difference: Reset, which sends `null` with no colour check on the short default.
  await userEvent.click(field("edge").getByRole("button", { name: "Reset to default" }));
  expect(field("edge").queryByText(/^Use a colour like #/)).toBeNull();
  await save();
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ edge: null }));
});

test("a_cleared_number_is_not_a_reset", async () => {
  const { onSubmit } = await setup();
  const columns = screen.getByLabelText("Columns");
  await userEvent.clear(columns);
  expect(field("columns").getByText("Enter a number.")).toBeInTheDocument();
  // Save never returns silently: the field takes the focus and a toast says why.
  await userEvent.click(screen.getByLabelText("Title"));
  await save();
  expect(onSubmit).not.toHaveBeenCalled();
  expect(columns).toHaveFocus();
});
