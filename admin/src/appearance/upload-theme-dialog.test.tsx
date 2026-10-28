import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ThemeDto } from "@/api/themes";
import { renderWithRouter } from "@/test/render";
import { UploadThemeDialog } from "./upload-theme-dialog";

class FakeXhr {
  static last: FakeXhr | null = null;
  aborted = false;
  upload = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  withCredentials = false;
  constructor() {
    FakeXhr.last = this;
  }
  open() {}
  setRequestHeader() {}
  send() {}
  abort() {
    this.aborted = true;
    this.onabort?.();
  }
}

beforeEach(() => {
  FakeXhr.last = null;
  vi.stubGlobal("XMLHttpRequest", FakeXhr);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

function theme(over: Partial<ThemeDto>): ThemeDto {
  return {
    name: "blog",
    version: "0.2.0",
    origin: "installed",
    active: false,
    has_settings: false,
    menus: [],
    images: [],
    nodes: [],
    ...over,
  };
}

function picked(path: string, body: string): File {
  const file = new File([body], path.slice(path.lastIndexOf("/") + 1));
  Object.defineProperty(file, "webkitRelativePath", { value: path });
  return file;
}

const TOML = '[theme]\nname = "blog"\nversion = "1.0.0"\n';

function folderInput(): HTMLInputElement {
  const input = document.querySelector<HTMLInputElement>("[data-slot=theme-folder-input]");
  if (!input) throw new Error("no folder input");
  return input;
}

test("replacing_the_active_theme_says_it_goes_live", async () => {
  renderWithRouter(
    <UploadThemeDialog
      open
      onOpenChange={() => {}}
      themes={[theme({ active: true })]}
      onInstalled={() => {}}
    />,
  );
  await screen.findByRole("dialog", { name: "Upload a theme" });
  await userEvent.upload(folderInput(), [picked("blog/theme.toml", TOML)]);
  expect(await screen.findByText(/it is the active theme/)).toHaveTextContent(
    "A theme named blog is installed (version 0.2.0) and it is the active theme. " +
      "Uploading replaces it, and the new files go live on the site at once.",
  );
  expect(screen.getByRole("button", { name: "Replace theme" })).toBeEnabled();
});

test("pack_error_names_the_file", async () => {
  renderWithRouter(
    <UploadThemeDialog open onOpenChange={() => {}} themes={[]} onInstalled={() => {}} />,
  );
  await screen.findByRole("dialog", { name: "Upload a theme" });
  await userEvent.upload(folderInput(), [
    picked("t/theme.toml", TOML),
    picked("t/assets/a\\b.css", "x"),
  ]);
  await userEvent.click(await screen.findByRole("button", { name: "Upload" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "assets/a\\b.css: the path has a backslash, a control character or an empty segment.",
  );
  // Nothing was sent.
  expect(FakeXhr.last).toBeNull();
});

function Harness() {
  const [open, setOpen] = useState(true);
  return (
    <UploadThemeDialog open={open} onOpenChange={setOpen} themes={[]} onInstalled={() => {}} />
  );
}

test("closing_aborts_the_upload", async () => {
  renderWithRouter(<Harness />);
  await screen.findByRole("dialog", { name: "Upload a theme" });
  await userEvent.upload(folderInput(), [picked("t/theme.toml", TOML)]);
  await userEvent.click(await screen.findByRole("button", { name: "Upload" }));
  await waitFor(() => expect(FakeXhr.last).not.toBeNull());
  expect(screen.getByRole("progressbar", { name: "Upload progress" })).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(FakeXhr.last?.aborted).toBe(true));
});
