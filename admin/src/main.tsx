// Must stay the first import: it configures Zod before any schema is built.
import "./lib/zod";
import "./styles/globals.css";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { toast } from "sonner";
import { createQueryClient, currentPath } from "./api/client";
import { App } from "./app";
import { holdResetToken, isResetPath, takeResetToken } from "./auth/reset-token";
import { DensityProvider } from "./design/density";
import { applyStoredPreferences } from "./design/preferences";
import { ThemeProvider } from "./design/theme";
import { initI18n } from "./i18n";
import { createAppRouter } from "./router";

// The password-reset link carries its token in the fragment: take it and drop it from
// the address bar before the router reads the location or anything sends a request
// (Review Focus 1). The reset page takes it from the holder once.
if (isResetPath(window.location.pathname)) {
  holdResetToken(takeResetToken(window.location, window.history));
}

// Theme and density before the first render, so the first frame is right (11 §3, §4).
applyStoredPreferences();

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
        <ThemeProvider>
          <DensityProvider>
            {/* Reduced motion turns off transform and layout animation (11 §5). */}
            <MotionConfig reducedMotion="user">
              <App queryClient={queryClient} router={router} />
            </MotionConfig>
          </DensityProvider>
        </ThemeProvider>
      </StrictMode>,
    );
  }
});
