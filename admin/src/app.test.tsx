import { QueryClient } from "@tanstack/react-query";
import { createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { expect, test } from "vitest";
import { App } from "./app";
import { i18n } from "./i18n";
import { createAppRouter } from "./router";

test("renders_root_with_translated_title", async () => {
  const queryClient = new QueryClient();
  const history = createMemoryHistory({ initialEntries: ["/admin/"] });
  const router = createAppRouter(queryClient, history);

  render(<App queryClient={queryClient} router={router} />);

  const name = i18n.t("app.name");
  expect(name).toBe("FerriteCMS");
  expect(await screen.findByRole("heading", { level: 1, name })).toBeInTheDocument();
  await waitFor(() => {
    expect(document.title).toBe(name);
  });
});
