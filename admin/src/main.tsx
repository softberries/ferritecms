// Must stay the first import: it configures Zod before any schema is built.
import "./lib/zod";
import "./index.css";
import { QueryClient } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app";
import { initI18n } from "./i18n";
import { createAppRouter } from "./router";

const queryClient = new QueryClient();
const router = createAppRouter(queryClient);

void initI18n().then(() => {
  const root = document.getElementById("root");
  if (root) {
    createRoot(root).render(
      <StrictMode>
        <App queryClient={queryClient} router={router} />
      </StrictMode>,
    );
  }
});
