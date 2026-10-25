// Per-viewer display preferences (docs/design/11-admin-ui.md §3, §4): the colour theme
// and the density, kept in `localStorage`. Storage can be missing or throw (private
// windows, blocked site data), so every access is guarded and falls back to the default.

export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";
export type Density = "comfortable" | "compact";

export const THEME_KEY = "ferrite.admin.theme";
export const DENSITY_KEY = "ferrite.admin.density";
export const DARK_QUERY = "(prefers-color-scheme: dark)";

const THEMES: readonly ThemePreference[] = ["light", "dark", "system"];
const DENSITIES: readonly Density[] = ["comfortable", "compact"];

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Not persisted; the choice still holds for this page.
  }
}

export function storedTheme(): ThemePreference {
  const v = read(THEME_KEY);
  return THEMES.find((t) => t === v) ?? "system";
}

export function storeTheme(theme: ThemePreference): void {
  write(THEME_KEY, theme);
}

export function storedDensity(): Density {
  const v = read(DENSITY_KEY);
  return DENSITIES.find((d) => d === v) ?? "comfortable";
}

export function storeDensity(density: Density): void {
  write(DENSITY_KEY, density);
}

/** The system scheme; light where `matchMedia` is unavailable. */
export function systemTheme(): ResolvedTheme {
  return typeof window.matchMedia === "function" && window.matchMedia(DARK_QUERY).matches
    ? "dark"
    : "light";
}

export function resolveTheme(theme: ThemePreference): ResolvedTheme {
  return theme === "system" ? systemTheme() : theme;
}

/** Toggles `.dark` on <html> (tokens.css and the `dark:` variant key off it). */
export function applyTheme(theme: ResolvedTheme): void {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

/** Sets `html[data-density]` (tokens.css switches the density variables on it). */
export function applyDensity(density: Density): void {
  document.documentElement.dataset.density = density;
}

/**
 * Applies the stored preferences synchronously. `main.tsx` calls it as module code
 * before the first render, so the first frame already has the right theme (no inline
 * script: the CSP forbids one).
 */
export function applyStoredPreferences(): void {
  applyTheme(resolveTheme(storedTheme()));
  applyDensity(storedDensity());
}
