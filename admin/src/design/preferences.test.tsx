import { act, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DensityProvider, useDensity } from "./density";
import {
  applyStoredPreferences,
  DENSITY_KEY,
  SIDEBAR_KEY,
  storeSidebarOpen,
  storedSidebarOpen,
  THEME_KEY,
  writeThemeCookie,
} from "./preferences";
import { ThemeProvider, useTheme } from "./theme";

/** A controllable `prefers-color-scheme: dark` query. */
function stubSystemScheme(dark: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    matches: dark,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
  };
  vi.stubGlobal("matchMedia", () => query);
  return {
    set(next: boolean) {
      query.matches = next;
      listeners.forEach((fn) => fn());
    },
  };
}

function ThemeProbe() {
  const { theme, resolvedTheme, setTheme } = useTheme();
  return (
    <>
      <output>{`${theme}/${resolvedTheme}`}</output>
      <button type="button" onClick={() => setTheme("dark")}>
        dark
      </button>
      <button type="button" onClick={() => setTheme("light")}>
        light
      </button>
    </>
  );
}

function SystemButton() {
  const { setTheme } = useTheme();
  return (
    <button type="button" onClick={() => setTheme("system")}>
      system
    </button>
  );
}

const html = document.documentElement;

beforeEach(() => {
  localStorage.clear();
  html.className = "";
  delete html.dataset.density;
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("theme", () => {
  it("system_theme_follows_the_media_query", () => {
    const scheme = stubSystemScheme(false);
    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("system/light");
    // Exactly one class once JS runs (tokens.css's no-JS fallback keys off neither).
    expect(html.className).toBe("light");
    act(() => scheme.set(true));
    expect(screen.getByRole("status")).toHaveTextContent("system/dark");
    expect(html.className).toBe("dark");
  });

  it("chosen_theme_is_stored_and_wins_over_the_system", () => {
    const scheme = stubSystemScheme(true);
    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );
    act(() => screen.getByRole("button", { name: "light" }).click());
    expect(localStorage.getItem(THEME_KEY)).toBe("light");
    expect(html).not.toHaveClass("dark");
    act(() => scheme.set(false));
    act(() => scheme.set(true));
    expect(html).not.toHaveClass("dark");
    act(() => screen.getByRole("button", { name: "dark" }).click());
    expect(html).toHaveClass("dark");
  });

  it("stored_preferences_apply_before_render", () => {
    stubSystemScheme(false);
    localStorage.setItem(THEME_KEY, "dark");
    localStorage.setItem(DENSITY_KEY, "compact");
    applyStoredPreferences();
    expect(html.className).toBe("dark");
    expect(html.dataset.density).toBe("compact");
  });

  it("theme_cookie_is_written_for_a_choice_and_removed_for_system", () => {
    // P57: the server turns the cookie into the <html> class before any JS runs.
    stubSystemScheme(false);
    const writes: string[] = [];
    vi.spyOn(Document.prototype, "cookie", "set").mockImplementation((v: string) => {
      writes.push(v);
    });
    render(
      <ThemeProvider>
        <ThemeProbe />
        <SystemButton />
      </ThemeProvider>,
    );
    act(() => screen.getByRole("button", { name: "dark" }).click());
    act(() => screen.getByRole("button", { name: "light" }).click());
    act(() => screen.getByRole("button", { name: "system" }).click());
    expect(writes).toEqual([
      "ferrite_admin_theme=dark; Path=/admin; SameSite=Lax; Max-Age=31536000",
      "ferrite_admin_theme=light; Path=/admin; SameSite=Lax; Max-Age=31536000",
      "ferrite_admin_theme=; Path=/admin; SameSite=Lax; Max-Age=0",
    ]);
    // On https the cookie is Secure.
    writes.length = 0;
    vi.stubGlobal("location", { ...window.location, protocol: "https:" });
    writeThemeCookie("dark");
    expect(writes).toEqual([
      "ferrite_admin_theme=dark; Path=/admin; SameSite=Lax; Max-Age=31536000; Secure",
    ]);
  });

  it("stored_choice_refreshes_the_cookie_on_load", () => {
    stubSystemScheme(false);
    const writes: string[] = [];
    vi.spyOn(Document.prototype, "cookie", "set").mockImplementation((v: string) => {
      writes.push(v);
    });
    localStorage.setItem(THEME_KEY, "dark");
    applyStoredPreferences();
    expect(writes).toEqual([
      "ferrite_admin_theme=dark; Path=/admin; SameSite=Lax; Max-Age=31536000",
    ]);
  });

  it("unknown_or_unreadable_storage_falls_back_to_defaults", () => {
    stubSystemScheme(false);
    localStorage.setItem(THEME_KEY, "sepia");
    applyStoredPreferences();
    expect(html).not.toHaveClass("dark");
    expect(html.dataset.density).toBe("comfortable");

    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("system/light");
    act(() => screen.getByRole("button", { name: "dark" }).click());
    expect(html).toHaveClass("dark");
  });

  it("works_without_match_media", () => {
    vi.stubGlobal("matchMedia", undefined);
    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("system/light");
  });

  it("hooks_need_their_provider", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => renderHook(() => useTheme())).toThrow(/ThemeProvider/);
    expect(() => renderHook(() => useDensity())).toThrow(/DensityProvider/);
  });
});

describe("density", () => {
  it("density_sets_the_attribute_and_is_stored", () => {
    const { result } = renderHook(() => useDensity(), { wrapper: DensityProvider });
    expect(result.current.density).toBe("comfortable");
    expect(html.dataset.density).toBe("comfortable");
    act(() => result.current.setDensity("compact"));
    expect(html.dataset.density).toBe("compact");
    expect(localStorage.getItem(DENSITY_KEY)).toBe("compact");
  });
});

describe("sidebar", () => {
  it("sidebar_state_is_stored_per_viewer", () => {
    expect(storedSidebarOpen()).toBe(true);
    storeSidebarOpen(false);
    expect(localStorage.getItem(SIDEBAR_KEY)).toBe("collapsed");
    expect(storedSidebarOpen()).toBe(false);
    storeSidebarOpen(true);
    expect(storedSidebarOpen()).toBe(true);
    // Blocked storage: the default, and no throw.
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(storedSidebarOpen()).toBe(true);
    expect(() => storeSidebarOpen(false)).not.toThrow();
  });
});
