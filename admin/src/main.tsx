// Must stay the first import: it configures Zod before any schema is built.
import "./lib/zod";
import "./index.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { toast } from "sonner";
import { createQueryClient, currentPath } from "./api/client";
import { App } from "./app";
import { initI18n } from "./i18n";
import { createAppRouter } from "./router";

// The router needs the query client and the client's error handler needs the
// router; the handler only runs after both exist.
const queryClient = createQueryClient({
  // Router navigation, past any leave guard, never a full-page load (F37).
  navigate: (href) => router.navigate({ href, replace: true, ignoreBlocker: true }),
  currentPath,
  notify: (id, message) => toast.warning(message, { id }),
});
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
