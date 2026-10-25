import "@/lib/zod";
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, expect } from "vitest";
import { initI18n } from "@/i18n";
import { missingKeys } from "./missing-keys";

// jsdom does not implement scrolling; the router's scroll restoration calls it.
window.scrollTo = () => {};

// A missing translation key is a test failure: `t()` throws at the call, and a
// key swallowed by an error boundary still fails the test in `afterEach`.
await initI18n({
  onMissingKey: (ns, key) => {
    const name = `${ns}:${key}`;
    missingKeys.push(name);
    throw new Error(`missing i18n key: ${name}`);
  },
});

afterEach(() => {
  cleanup();
  const missing = missingKeys.splice(0);
  expect(missing, "missing i18n keys").toEqual([]);
});
