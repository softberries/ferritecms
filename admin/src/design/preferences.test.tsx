import { act, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DensityProvider, useDensity } from "./density";
import { applyStoredPreferences, DENSITY_KEY, THEME_KEY } from "./preferences";
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
    expect(html).not.toHaveClass("dark");
    act(() => scheme.set(true));
    expect(screen.getByRole("status")).toHaveTextContent("system/dark");
    expect(html).toHaveClass("dark");
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
    expect(html).toHaveClass("dark");
    expect(html.dataset.density).toBe("compact");
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
