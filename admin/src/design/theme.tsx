// The colour theme: light, dark or the system's (docs/design/11-admin-ui.md §3).
// No next-themes (M2b P9); the shadcn sonner wrapper reads `useTheme` from here.

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  DARK_QUERY,
  applyTheme,
  storeTheme,
  storedTheme,
  systemTheme,
  type ResolvedTheme,
  type ThemePreference,
} from "./preferences";

export type { ResolvedTheme, ThemePreference };

export interface ThemeState {
  /** The viewer's choice. */
  theme: ThemePreference;
  /** What is on screen: the choice, or the system scheme under `system`. */
  resolvedTheme: ResolvedTheme;
  setTheme: (theme: ThemePreference) => void;
}

const ThemeContext = createContext<ThemeState | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemePreference>(storedTheme);
  const [system, setSystem] = useState<ResolvedTheme>(systemTheme);

  // Follow the system scheme live (it matters only under `system`).
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(DARK_QUERY);
    const onChange = () => setSystem(query.matches ? "dark" : "light");
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  const resolvedTheme = theme === "system" ? system : theme;
  useEffect(() => applyTheme(resolvedTheme), [resolvedTheme]);

  const setTheme = useCallback((next: ThemePreference) => {
    storeTheme(next);
    setThemeState(next);
  }, []);

  const value = useMemo(
    () => ({ theme, resolvedTheme, setTheme }),
    [theme, resolvedTheme, setTheme],
  );
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeState {
  const state = useContext(ThemeContext);
  if (!state) throw new Error("useTheme needs a <ThemeProvider>");
  return state;
}
