// Per-viewer display preferences (docs/design/11-admin-ui.md §3, §4, §7): the colour
// theme, the density, the sidebar state and the editor panel's, kept in `localStorage`. Storage can be
// missing or throw (private windows, blocked site data), so every access is guarded and
// falls back to the default.

export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";
export type Density = "comfortable" | "compact";

export const THEME_KEY = "ferrite.admin.theme";
export const DENSITY_KEY = "ferrite.admin.density";
/** The shell's sidebar: `expanded` (default) or `collapsed` to the icon rail. */
export const SIDEBAR_KEY = "ferrite.admin.sidebar";
/** The editor's side panel on wide screens: `open` (default) or `closed`. */
export const EDITOR_PANEL_KEY = "ferrite.admin.editorPanel";
/** The media library's view: `grid` (default) or `list`. */
export const MEDIA_VIEW_KEY = "ferrite.admin.mediaView";
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

/**
 * The cookie `index.html` serving turns into `class="dark"` / `class="light"` on <html>
 * (P57), so an explicit choice paints before any JS. Written for `light` and `dark`,
 * removed for `system`. Not `HttpOnly`: this code writes it.
 */
export const THEME_COOKIE = "ferrite_admin_theme";
const THEME_COOKIE_MAX_AGE = 60 * 60 * 24 * 365;

export function writeThemeCookie(theme: ThemePreference): void {
  const secure = window.location.protocol === "https:" ? "; Secure" : "";
  const value = theme === "system" ? "; Max-Age=0" : `; Max-Age=${THEME_COOKIE_MAX_AGE}`;
  const set = theme === "system" ? "" : theme;
  try {
    document.cookie = `${THEME_COOKIE}=${set}; Path=/admin; SameSite=Lax${value}${secure}`;
  } catch {
    // Cookies blocked: the class then comes from JS, one frame later.
  }
}

export function storeTheme(theme: ThemePreference): void {
  write(THEME_KEY, theme);
  writeThemeCookie(theme);
}

export function storedDensity(): Density {
  const v = read(DENSITY_KEY);
  return DENSITIES.find((d) => d === v) ?? "comfortable";
}

export function storeDensity(density: Density): void {
  write(DENSITY_KEY, density);
}

/** Whether the desktop sidebar is expanded (the default) rather than an icon rail. */
export function storedSidebarOpen(): boolean {
  return read(SIDEBAR_KEY) !== "collapsed";
}

export function storeSidebarOpen(open: boolean): void {
  write(SIDEBAR_KEY, open ? "expanded" : "collapsed");
}

/** Whether the editor's side panel is open on wide screens (the default). */
export function storedEditorPanelOpen(): boolean {
  return read(EDITOR_PANEL_KEY) !== "closed";
}

export function storeEditorPanelOpen(open: boolean): void {
  write(EDITOR_PANEL_KEY, open ? "open" : "closed");
}

export function storedMediaView(): "grid" | "list" {
  return read(MEDIA_VIEW_KEY) === "list" ? "list" : "grid";
}

export function storeMediaView(view: "grid" | "list"): void {
  write(MEDIA_VIEW_KEY, view);
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

/**
 * Sets exactly one of `.light` / `.dark` on <html> (tokens.css and the `dark:` variant
 * key off `.dark`). No class at all means JS has not run yet; tokens.css then follows
 * the system scheme on its own.
 */
export function applyTheme(theme: ResolvedTheme): void {
  const classes = document.documentElement.classList;
  classes.toggle("dark", theme === "dark");
  classes.toggle("light", theme === "light");
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
  const theme = storedTheme();
  applyTheme(resolveTheme(theme));
  // Keeps the server hint in step with storage (a choice made before P57, or a cookie
  // that expired or was cleared on its own).
  writeThemeCookie(theme);
  applyDensity(storedDensity());
}
